//! Club map ingest load test: 50 active tails at 1 Hz, assert append p95.
#[cfg(test)]
mod tests {
    use crate::club::{db::ClubDb, now_ms, visibility::FlightVisibility};
    use crate::live::LiveFix;
    use serial_test::serial;
    use std::time::Instant;

    async fn fixture() -> (ClubDb, Vec<(String, String)>) {
        let db = ClubDb::open_test().await.expect("db");
        let club = db.default_club_id().await.unwrap();
        db.upsert_type(&club, "C152").await.unwrap();
        let mut flights = Vec::new();
        for i in 0..50 {
            let tail = format!("N{i:03}");
            db.upsert_aircraft(&club, &tail, "C152", true)
                .await
                .unwrap();
            let pid = db
                .register(
                    &club,
                    &format!("P{i}"),
                    &format!("p{i}@load.test"),
                    "",
                    "password12",
                )
                .await
                .unwrap();
            db.approve_pilot(&pid).await.unwrap();
            db.add_checkout(&pid, "C152", None, None).await.unwrap();
            let (dev, _) = db.create_device(&pid, "phone").await.unwrap();
            db.issue_open_checkin(&pid, &dev, &club, now_ms())
                .await
                .unwrap();
            let fid = db
                .start_flight(&pid, &dev, &tail, FlightVisibility::Club, false, now_ms())
                .await
                .unwrap();
            flights.push((fid, dev));
        }
        (db, flights)
    }

    #[serial]
    #[tokio::test]
    async fn fifty_tails_one_hz_append_p95_under_25ms() {
        let (db, flights) = fixture().await;
        let mut samples = Vec::with_capacity(50);
        let fix = LiveFix {
            t: now_ms(),
            lat: -33.4,
            lon: -70.6,
            alt: Some(1200.0),
            alt_ref: None,
            speed: Some(30.0),
            course: Some(90.0),
            accuracy: Some(8.0),
        };
        for (fid, dev) in &flights {
            let start = Instant::now();
            db.append_fixes(fid, dev, std::slice::from_ref(&fix))
                .await
                .expect("append");
            samples.push(start.elapsed());
        }
        samples.sort();
        let p95_idx = (samples.len() * 95 / 100).min(samples.len() - 1);
        let p95 = samples[p95_idx];
        assert!(
            p95.as_millis() <= 25,
            "append_fixes p95 {p95:?} exceeds 25ms"
        );
    }
}
