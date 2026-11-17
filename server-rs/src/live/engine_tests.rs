#[cfg(test)]
mod tests {
    use super::super::engine::{
        AckResult, LiveEngine, TestClock, MAX_FIXES_PER_SESSION, SESSION_IDLE_END_MS,
        SESSION_RETAIN_MS,
    };
    use super::super::persist::LivePersist;
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
        let base = 1_700_000_000_000_i64;
        let fixes = vec![
            sample_fix(base + 3000, -33.4),
            sample_fix(base + 1000, -33.41),
            sample_fix(base + 2000, -33.42),
            sample_fix(base + 2000, -33.99),
        ];
        let n = engine.ingest_fixes("dev", fixes).await.unwrap();
        assert_eq!(n, 3);
        let sessions = engine.list_sessions().await;
        assert_eq!(sessions.len(), 1);
        let id = sessions[0].id.clone();
        let stored = engine.get_fixes(&id).await.unwrap();
        assert_eq!(stored.len(), 3);
        assert_eq!(stored[0].t, base + 1000);
        assert_eq!(stored[2].t, base + 3000);
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
        assert_eq!(engine2.ack_session(&id2).await, AckResult::Ok);
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
    async fn fix_cap_rolls_to_new_session() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let start = 1_700_000_000_000;
        let (engine, _) = engine_at(start, &dir);
        let mut total = 0usize;
        for chunk_start in (0..=MAX_FIXES_PER_SESSION).step_by(120) {
            let chunk_end = (chunk_start + 120).min(MAX_FIXES_PER_SESSION + 1);
            let batch: Vec<_> = (chunk_start..chunk_end)
                .map(|i| {
                    sample_fix(
                        start - (MAX_FIXES_PER_SESSION as i64 - i as i64) * 1000,
                        -33.0,
                    )
                })
                .collect();
            total += engine.ingest_fixes("dev", batch).await.unwrap();
        }
        assert_eq!(total, MAX_FIXES_PER_SESSION + 1);
        let listed = engine.list_sessions().await;
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|s| s.phase == SessionPhase::Active));
    }

    #[tokio::test]
    async fn backlog_drain_stays_one_session() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let start = 1_700_000_000_000;
        let (engine, clock) = engine_at(start, &dir);
        engine
            .ingest_fixes("dev", vec![sample_fix(start - 15 * 60 * 1000, -33.0)])
            .await
            .unwrap();
        clock.advance(15 * 60 * 1000);
        engine
            .ingest_fixes(
                "dev",
                vec![
                    sample_fix(start - 14 * 60 * 1000, -33.1),
                    sample_fix(start - 13 * 60 * 1000, -33.2),
                ],
            )
            .await
            .unwrap();
        engine.tick_idle_and_prune().await;
        let sessions = engine.list_sessions().await;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].phase, SessionPhase::Active);
        assert!(engine.get_fixes(&sessions[0].id).await.unwrap().len() >= 3);
    }

    #[tokio::test]
    async fn ack_on_active_session_rejected_and_ingest_continues() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let start = 1_700_000_000_000;
        let (engine, _) = engine_at(start, &dir);
        engine
            .ingest_fixes("dev", vec![sample_fix(start, -33.4)])
            .await
            .unwrap();
        let id = engine.list_sessions().await[0].id.clone();
        assert_eq!(engine.ack_session(&id).await, AckResult::StillActive);
        engine
            .ingest_fixes("dev", vec![sample_fix(start + 1000, -33.41)])
            .await
            .unwrap();
        assert_eq!(engine.get_fixes(&id).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn empty_index_recovers_orphan_logs_as_ended() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let start = 1_700_000_000_000;
        let (engine, _) = engine_at(start, &dir);
        let session_id = "abc123deadbeef01";
        let persist = LivePersist::new(&dir);
        persist.ensure_dir().await.unwrap();
        persist.write_raw_index(b"[]").await.unwrap();
        let fix = sample_fix(start, -33.4);
        persist.write_fix_line(session_id, &fix).await.unwrap();
        engine.load_from_disk().await.unwrap();
        let sessions = engine.list_sessions().await;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].phase, SessionPhase::Ended);
        assert_eq!(sessions[0].device, "recovered");
        assert_eq!(engine.get_fixes(session_id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn corrupt_index_recovers_device_from_broken_file() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let start = 1_700_000_000_000;
        let (engine, _) = engine_at(start, &dir);
        let session_id = "def456deadbeef02";
        let persist = LivePersist::new(&dir);
        persist.ensure_dir().await.unwrap();
        let broken = serde_json::json!([{
            "id": session_id,
            "device": "pilot-phone",
            "phase": "active",
            "started_at": start,
            "ended_at": null,
            "last_fix_at": start,
            "last_rx_at": start,
            "fix_count": 1,
            "acked": false
        }]);
        tokio::fs::write(
            dir.join("live/sessions.json.broken"),
            serde_json::to_string(&broken).unwrap(),
        )
        .await
        .unwrap();
        persist.write_raw_index(b"not valid json").await.unwrap();
        persist
            .write_fix_line(session_id, &sample_fix(start, -33.5))
            .await
            .unwrap();
        engine.load_from_disk().await.unwrap();
        let sessions = engine.list_sessions().await;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].device, "pilot-phone");
    }

    #[tokio::test]
    async fn stream_snapshot_is_atomic_with_phase_and_fixes() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let start = 1_700_000_000_000;
        let (engine, _) = engine_at(start, &dir);
        engine
            .ingest_fixes("dev", vec![sample_fix(start, -33.4)])
            .await
            .unwrap();
        let id = engine.list_sessions().await[0].id.clone();
        let snap = engine.stream_snapshot(&id).await.unwrap();
        assert_eq!(snap.phase, SessionPhase::Active);
        assert_eq!(snap.fixes.len(), 1);
        assert!(snap.live_rx.is_some());

        engine
            .ingest_fixes("dev", vec![sample_fix(start + 1000, -33.41)])
            .await
            .unwrap();
        let snap = engine.stream_snapshot(&id).await.unwrap();
        assert_eq!(snap.fixes.len(), 2);

        engine.stop_session(&id).await;
        let snap = engine.stream_snapshot(&id).await.unwrap();
        assert_eq!(snap.phase, SessionPhase::Ended);
        assert_eq!(snap.fixes.len(), 2);
        assert!(snap.live_rx.is_none());
    }

    #[tokio::test]
    async fn stream_snapshot_sees_end_before_subscribe_would_race() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let start = 1_700_000_000_000;
        let (engine, _) = engine_at(start, &dir);
        engine
            .ingest_fixes("dev", vec![sample_fix(start, -33.4)])
            .await
            .unwrap();
        let id = engine.list_sessions().await[0].id.clone();
        engine.stop_session(&id).await;
        let snap = engine.stream_snapshot(&id).await.unwrap();
        assert_eq!(snap.phase, SessionPhase::Ended);
        assert!(snap.live_rx.is_none());
    }
}
