//! Club map ingest load test: 50 active tails at 1 Hz, assert append p95.
#[cfg(test)]
mod tests {
    use crate::club::{db::ClubDb, now_ms, visibility::FlightVisibility};
    use crate::live::LiveFix;
    use std::time::Instant;

    fn fixture() -> (ClubDb, Vec<(String, String)>) {
        let db = ClubDb::open_in_memory().expect("db");
        let club = db.default_club_id().unwrap();
        db.upsert_type(&club, "C152").unwrap();
        let mut flights = Vec::new();
        for i in 0..50 {
            let tail = format!("N{i:03}");
            db.upsert_aircraft(&club, &tail, "C152", true).unwrap();
            let pid = db
                .register(
                    &club,
                    &format!("P{i}"),
                    &format!("p{i}@load.test"),
                    "",
                    "password12",
                )
                .unwrap();
            db.approve_pilot(&pid).unwrap();
            db.add_checkout(&pid, "C152", None, None).unwrap();
            let (dev, _) = db.create_device(&pid, "phone").unwrap();
            db.issue_open_checkin(&pid, &dev, &club, now_ms()).unwrap();
            let fid = db
                .start_flight(&pid, &dev, &tail, FlightVisibility::Club, false, now_ms())
                .unwrap();
            flights.push((fid, dev));
        }
        (db, flights)
    }

    #[test]
    fn fifty_tails_one_hz_append_p95_under_25ms() {
        let (db, flights) = fixture();
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
