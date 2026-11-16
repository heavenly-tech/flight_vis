#[cfg(test)]
mod tests {
    use super::super::engine::{
        LiveEngine, TestClock, MAX_FIXES_PER_SESSION, SESSION_IDLE_END_MS, SESSION_RETAIN_MS,
    };
    use super::super::types::{LiveFix, SessionPhase, ALT_REF_WGS84_ELLIPSOID};
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> std::path::PathBuf {
        let n = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "fv-live-{}-{}-{n}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn engine_at(start_ms: i64, data_dir: &std::path::Path) -> (Arc<LiveEngine>, Arc<TestClock>) {
        let clock = TestClock::new(start_ms);
        let tick = Arc::clone(&clock);
        let now = Arc::new(move || tick.0.load(Ordering::SeqCst));
        let engine = Arc::new(LiveEngine::with_clock(data_dir, now));
        (engine, clock)
    }

    fn sample_fix(t: i64, lat: f64) -> LiveFix {
        LiveFix {
            t,
            lat,
            lon: -70.6,
            alt: Some(800.0),
            alt_ref: Some(ALT_REF_WGS84_ELLIPSOID.to_string()),
            speed: Some(5.0),
            course: Some(90.0),
            accuracy: Some(12.0),
        }
    }

    #[tokio::test]
    async fn dedupe_and_out_of_order_sort() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let (engine, _clock) = engine_at(1_700_000_000_000, &dir);
        let fixes = vec![
            sample_fix(3000, -33.4),
            sample_fix(1000, -33.41),
            sample_fix(2000, -33.42),
            sample_fix(2000, -33.99),
        ];
        let n = engine.ingest_fixes("dev", fixes).await.unwrap();
        assert_eq!(n, 3);
        let sessions = engine.list_sessions().await;
        assert_eq!(sessions.len(), 1);
        let id = sessions[0].id.clone();
        let stored = engine.get_fixes(&id).await.unwrap();
        assert_eq!(stored.len(), 3);
        assert_eq!(stored[0].t, 1000);
        assert_eq!(stored[2].t, 3000);
        assert!((stored[1].lat + 33.42).abs() < 0.001);
    }

    #[tokio::test]
    async fn auto_end_after_ten_minutes_idle() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let start = 1_700_000_000_000;
        let (engine, clock) = engine_at(start, &dir);
        engine
            .ingest_fixes("dev", vec![sample_fix(start, -33.4)])
            .await
            .unwrap();
        let id = engine.list_sessions().await[0].id.clone();
        clock.advance(SESSION_IDLE_END_MS - 1);
        engine.tick_idle_and_prune().await;
        assert_eq!(engine.list_sessions().await[0].phase, SessionPhase::Active);
        clock.advance(2);
        engine.tick_idle_and_prune().await;
        let listed = engine.list_sessions().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].phase, SessionPhase::Ended);
        assert!(engine.get_fixes(&id).await.is_some());
    }

    #[tokio::test]
    async fn prune_after_24h_or_ack() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let start = 1_700_000_000_000;
        let (engine, clock) = engine_at(start, &dir);
        engine
            .ingest_fixes("dev", vec![sample_fix(start, -33.4)])
            .await
            .unwrap();
        let id = engine.list_sessions().await[0].id.clone();
        clock.advance(SESSION_IDLE_END_MS + 1);
        engine.tick_idle_and_prune().await;
        clock.advance(SESSION_RETAIN_MS);
        engine.tick_idle_and_prune().await;
        assert!(engine.get_fixes(&id).await.is_none());

        let (engine2, clock2) = engine_at(start, &dir);
        engine2
            .ingest_fixes("dev", vec![sample_fix(start, -33.5)])
            .await
            .unwrap();
        let id2 = engine2.list_sessions().await[0].id.clone();
        clock2.advance(SESSION_IDLE_END_MS + 1);
        engine2.tick_idle_and_prune().await;
        assert!(engine2.ack_session(&id2).await);
        engine2.tick_idle_and_prune().await;
        assert!(engine2.get_fixes(&id2).await.is_none());
    }

    #[tokio::test]
    async fn reload_from_disk_after_restart() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let start = 1_700_000_000_000;
        {
            let (engine, _) = engine_at(start, &dir);
            engine
                .ingest_fixes(
                    "phone",
                    vec![sample_fix(start, -33.4), sample_fix(start + 1000, -33.41)],
                )
                .await
                .unwrap();
        }
        let (engine2, _) = engine_at(start, &dir);
        engine2.load_from_disk().await.unwrap();
        let sessions = engine2.list_sessions().await;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].fix_count, 2);
        let fixes = engine2.get_fixes(&sessions[0].id).await.unwrap();
        assert_eq!(fixes.len(), 2);
    }

    #[tokio::test]
    async fn rejects_future_timestamp() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let start = 1_700_000_000_000;
        let (engine, _clock) = engine_at(start, &dir);
        let bad = sample_fix(start + 120_000, -33.4);
        assert!(engine.ingest_fixes("dev", vec![bad]).await.is_err());
    }

    #[tokio::test]
    async fn fix_cap_per_session() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let (engine, _) = engine_at(1_000_000, &dir);
        let mut batch = Vec::new();
        for i in 0..=MAX_FIXES_PER_SESSION {
            batch.push(sample_fix(i as i64 * 1000, -33.0));
        }
        let err = engine.ingest_fixes("dev", batch).await;
        assert!(err.is_err() || engine.list_sessions().await[0].fix_count <= MAX_FIXES_PER_SESSION);
    }
}
