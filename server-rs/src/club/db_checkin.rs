use crate::club::{
    auth::new_id,
    db::{now_ms, CheckinRecord, ClubDb, StartFlightError},
};
use rusqlite::{params, OptionalExtension};

pub const DEFAULT_CHECKIN_WINDOW_HOURS: i64 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtendCheckinError {
    Forbidden,
    NotFound,
    InvalidExpiry,
    Db,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetCheckinWindowError {
    Forbidden,
    Invalid,
    Db,
}

impl ClubDb {
    pub fn club_checkin_window_hours(&self, club_id: &str) -> Result<i64, rusqlite::Error> {
        let hours: i64 = self.with_conn(|conn| {
            conn.query_row(
                "SELECT checkin_window_hours FROM clubs WHERE id = ?1",
                params![club_id],
                |r| r.get(0),
            )
        })?;
        Ok(hours.max(1))
    }

    pub fn set_club_checkin_window_hours(
        &self,
        club_id: &str,
        hours: i64,
    ) -> Result<(), rusqlite::Error> {
        if hours < 1 || hours > 24 * 365 {
            return Err(rusqlite::Error::InvalidParameterName(
                "checkin_window_hours".into(),
            ));
        }
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE clubs SET checkin_window_hours = ?1 WHERE id = ?2",
                params![hours, club_id],
            )?;
            Ok(())
        })
    }

    pub fn admin_set_club_checkin_window(
        &self,
        admin_id: &str,
        hours: i64,
    ) -> Result<i64, SetCheckinWindowError> {
        if hours < 1 || hours > 24 * 365 {
            return Err(SetCheckinWindowError::Invalid);
        }
        let (club_id, role): (String, String) = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT club_id, role FROM pilots WHERE id = ?1",
                    params![admin_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
            })
            .map_err(|_| SetCheckinWindowError::Forbidden)?;
        if role != "admin" {
            return Err(SetCheckinWindowError::Forbidden);
        }
        self.set_club_checkin_window_hours(&club_id, hours)
            .map_err(|_| SetCheckinWindowError::Db)?;
        let _ = self.record_audit(
            &club_id,
            Some(admin_id),
            "club.checkin_window",
            "club",
            &club_id,
            Some(&format!(r#"{{"checkinWindowHours":{hours}}}"#)),
        );
        Ok(hours)
    }

    /// Non-consuming: an open check-in window for this pilot/device/club.
    pub fn has_open_checkin(
        &self,
        pilot_id: &str,
        device_id: &str,
        club_id: &str,
        now: i64,
    ) -> Result<bool, rusqlite::Error> {
        self.with_conn(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM checkins
                 WHERE outcome = 'ok' AND pilot_id = ?1 AND club_id = ?2
                   AND expires_at IS NOT NULL AND expires_at > ?3
                   AND (device_id IS NULL OR device_id = ?4)",
                params![pilot_id, club_id, now, device_id],
                |r| r.get(0),
            )?;
            Ok(count > 0)
        })
    }

    pub fn latest_open_checkin(
        &self,
        pilot_id: &str,
        club_id: &str,
        now: i64,
    ) -> Option<CheckinRecord> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT id, created_at, expires_at FROM checkins
                 WHERE outcome = 'ok' AND pilot_id = ?1 AND club_id = ?2
                   AND expires_at IS NOT NULL AND expires_at > ?3
                 ORDER BY expires_at DESC LIMIT 1",
                params![pilot_id, club_id, now],
                |r| {
                    Ok(CheckinRecord {
                        id: r.get(0)?,
                        created_at: r.get(1)?,
                        expires_at: r.get(2)?,
                    })
                },
            )
            .optional()
        })
        .ok()
        .flatten()
    }

    pub fn admin_extend_pilot_checkin(
        &self,
        admin_id: &str,
        pilot_id: &str,
        expires_at_ms: i64,
        now: i64,
    ) -> Result<CheckinRecord, ExtendCheckinError> {
        if expires_at_ms <= now {
            return Err(ExtendCheckinError::InvalidExpiry);
        }
        let (admin_club, admin_role) = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT club_id, role FROM pilots WHERE id = ?1",
                    params![admin_id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
            })
            .map_err(|_| ExtendCheckinError::Forbidden)?;
        if admin_role != "admin" {
            return Err(ExtendCheckinError::Forbidden);
        }
        let pilot_club = self
            .pilot_club_id(pilot_id)
            .map_err(|_| ExtendCheckinError::Forbidden)?;
        if pilot_club != admin_club {
            return Err(ExtendCheckinError::Forbidden);
        }
        let checkin_id: String = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT id FROM checkins
                     WHERE outcome = 'ok' AND pilot_id = ?1 AND club_id = ?2
                     ORDER BY created_at DESC LIMIT 1",
                    params![pilot_id, pilot_club],
                    |r| r.get(0),
                )
            })
            .map_err(|_| ExtendCheckinError::NotFound)?;
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE checkins SET expires_at = ?1 WHERE id = ?2",
                params![expires_at_ms, checkin_id],
            )?;
            Ok(())
        })
        .map_err(|_| ExtendCheckinError::Db)?;
        let rec = CheckinRecord {
            id: checkin_id.clone(),
            created_at: now,
            expires_at: expires_at_ms,
        };
        let _ = self.record_audit(
            &pilot_club,
            Some(admin_id),
            "checkin.extend",
            "checkin",
            &checkin_id,
            Some(&format!(
                r#"{{"pilotId":"{pilot_id}","expiresAtMs":{expires_at_ms}}}"#
            )),
        );
        Ok(rec)
    }

    pub fn start_flight_atomic(
        &self,
        pilot_id: &str,
        device_id: &str,
        tail: &str,
        visibility: crate::club::visibility::FlightVisibility,
        show_pilot_name_public: bool,
        now: i64,
    ) -> Result<String, StartFlightError> {
        let mut outcome: Result<String, StartFlightError> = Err(StartFlightError::NoTail);
        let db_err = self.with_conn(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            outcome = (|| -> Result<String, StartFlightError> {
                let dev_owner: Option<String> = conn
                    .query_row(
                        "SELECT pilot_id FROM devices WHERE id = ?1",
                        params![device_id],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|_| StartFlightError::DeviceMismatch)?;
                if dev_owner.as_deref() != Some(pilot_id) {
                    return Err(StartFlightError::DeviceMismatch);
                }
                let checkin_club: String = conn
                    .query_row(
                        "SELECT club_id FROM checkins
                         WHERE outcome = 'ok' AND pilot_id = ?1
                           AND expires_at IS NOT NULL AND expires_at > ?2
                           AND (device_id IS NULL OR device_id = ?3)
                         ORDER BY expires_at DESC LIMIT 1",
                        params![pilot_id, now, device_id],
                        |r| r.get(0),
                    )
                    .map_err(|_| StartFlightError::NoCheckin)?;
                let aircraft_id: Option<String> = conn
                    .query_row(
                        "SELECT id FROM aircraft WHERE tail = ?1 AND club_id = ?2 AND active = 1",
                        params![tail, checkin_club],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|_| StartFlightError::NoTail)?;
                let Some(aircraft_id) = aircraft_id else {
                    return Err(StartFlightError::NoTail);
                };
                let type_code: String = conn
                    .query_row(
                        "SELECT t.code FROM aircraft a
                         JOIN aircraft_types t ON t.id = a.type_id
                         WHERE a.id = ?1",
                        params![aircraft_id],
                        |r| r.get(0),
                    )
                    .map_err(|_| StartFlightError::NoTail)?;
                if !ClubDb::checkout_ok(conn, pilot_id, &aircraft_id, now)
                    .map_err(|_| StartFlightError::NoTail)?
                {
                    return Err(StartFlightError::NotCleared {
                        tail: tail.to_string(),
                        type_code,
                    });
                }
                let pilot_active: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM flights WHERE pilot_id = ?1 AND status = 'active'",
                        params![pilot_id],
                        |r| r.get(0),
                    )
                    .map_err(|_| StartFlightError::NoTail)?;
                if pilot_active > 0 {
                    return Err(StartFlightError::AlreadyActive);
                }
                let device_active: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM flights f
                         JOIN flight_devices fd ON fd.flight_id = f.id
                         WHERE fd.device_id = ?1 AND f.status = 'active'",
                        params![device_id],
                        |r| r.get(0),
                    )
                    .map_err(|_| StartFlightError::NoTail)?;
                if device_active > 0 {
                    return Err(StartFlightError::AlreadyActive);
                }
                let club_id = checkin_club;
                let checkins: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM checkins
                         WHERE outcome = 'ok' AND pilot_id = ?1 AND club_id = ?2
                           AND expires_at IS NOT NULL AND expires_at > ?3
                           AND (device_id IS NULL OR device_id = ?4)",
                        params![pilot_id, club_id, now, device_id],
                        |r| r.get(0),
                    )
                    .map_err(|_| StartFlightError::NoCheckin)?;
                if checkins == 0 {
                    return Err(StartFlightError::NoCheckin);
                }
                let flight_id = new_id();
                conn.execute(
                    "INSERT INTO flights (id, pilot_id, aircraft_id, club_id, status, started_at, visibility, show_pilot_name_public)
                     VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, ?7)",
                    params![
                        flight_id,
                        pilot_id,
                        aircraft_id,
                        club_id,
                        now,
                        visibility.as_str(),
                        i64::from(show_pilot_name_public)
                    ],
                )
                .map_err(|_| StartFlightError::NoTail)?;
                conn.execute(
                    "INSERT INTO flight_devices (flight_id, device_id) VALUES (?1, ?2)",
                    params![flight_id, device_id],
                )
                .map_err(|_| StartFlightError::NoTail)?;
                Ok(flight_id)
            })();
            if outcome.is_ok() {
                conn.execute_batch("COMMIT")?;
            } else {
                let _ = conn.execute_batch("ROLLBACK");
            }
            Ok(())
        });
        if db_err.is_err() {
            return Err(StartFlightError::NoTail);
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::club::{db::ClubDb, visibility::FlightVisibility};

    fn seed_with_window(db: &ClubDb, window_hours: i64) -> (String, String, String) {
        let club = db.default_club_id().unwrap();
        db.set_club_checkin_window_hours(&club, window_hours).unwrap();
        let type_id = db.upsert_type(&club, "C152").unwrap();
        let _ = type_id;
        db.upsert_aircraft(&club, "N123", "C152", true).unwrap();
        let a = db
            .register(&club, "Alice", "a@test", "L1", "password12")
            .unwrap();
        db.approve_pilot(&a).unwrap();
        db.add_checkout(&a, "C152", None, None).unwrap();
        let dev = db.create_device(&a, "phone").unwrap().0;
        (club, a, dev)
    }

    #[test]
    fn window_hours_apply_to_new_checkin() {
        let db = ClubDb::open_in_memory().unwrap();
        let (club, a, dev) = seed_with_window(&db, 3);
        let t = 1_800_000_000_000_i64;
        let token = db.mint_checkin_qr_token(&club, t).unwrap();
        let rec = db.submit_checkin(&a, Some(&dev), &token, t).unwrap();
        assert_eq!(rec.expires_at, t + 3 * 3_600_000);
    }

    #[test]
    fn admin_extension_after_default_window() {
        let db = ClubDb::open_in_memory().unwrap();
        let (club, a, dev) = seed_with_window(&db, 1);
        let admin = db
            .register(&club, "Admin", "adm@test", "", "password12")
            .unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE pilots SET role = 'admin', status = 'approved' WHERE id = ?1",
                params![admin],
            )
        })
        .unwrap();
        let t = 2_000_000_000_000_i64;
        db.issue_open_checkin(&a, &dev, &club, t).unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE checkins SET expires_at = ?1 WHERE pilot_id = ?2",
                params![t + 30 * 60_000, a],
            )
        })
        .unwrap();
        let after_window = t + 2 * 3_600_000;
        assert!(matches!(
            db.start_flight_atomic(
                &a,
                &dev,
                "N123",
                FlightVisibility::Club,
                false,
                after_window
            ),
            Err(StartFlightError::NoCheckin)
        ));
        let extended = after_window + 7 * 24 * 3_600_000;
        db.admin_extend_pilot_checkin(&admin, &a, extended, after_window)
            .unwrap();
        assert!(db
            .start_flight_atomic(
                &a,
                &dev,
                "N123",
                FlightVisibility::Club,
                false,
                after_window
            )
            .is_ok());
    }

    #[test]
    fn non_admin_cannot_extend_checkin() {
        let db = ClubDb::open_in_memory().unwrap();
        let (club, a, dev) = seed_with_window(&db, 2);
        let b = db
            .register(&club, "Bob", "b@test", "L2", "password12")
            .unwrap();
        db.approve_pilot(&b).unwrap();
        let t = now_ms();
        db.issue_open_checkin(&a, &dev, &club, t).unwrap();
        assert_eq!(
            db.admin_extend_pilot_checkin(&b, &a, t + 9_000_000, t),
            Err(ExtendCheckinError::Forbidden)
        );
    }

    #[test]
    fn two_starts_within_window_second_needs_end_first() {
        let db = ClubDb::open_in_memory().unwrap();
        let (club, a, dev) = seed_with_window(&db, 4);
        let t = now_ms();
        db.issue_open_checkin(&a, &dev, &club, t).unwrap();
        let f1 = db
            .start_flight_atomic(&a, &dev, "N123", FlightVisibility::Club, false, t)
            .unwrap();
        assert!(matches!(
            db.start_flight_atomic(&a, &dev, "N123", FlightVisibility::Club, false, t + 1),
            Err(StartFlightError::AlreadyActive)
        ));
        db.end_flight(&a, &f1, t + 2).unwrap();
        assert!(db
            .start_flight_atomic(
                &a,
                &dev,
                "N123",
                FlightVisibility::Club,
                false,
                t + 3
            )
            .is_ok());
    }
}