use crate::club::{
    auth::{self, new_id, digest_token, SESSION_TTL_SEC},
    map::{dedupe_by_tail, filter_admin_tails, DeviceFix, TailLivePosition},
    visibility::{
        can_view_flight, includes_in_club_map, includes_in_public_map, show_pilot_name,
        FlightVisibility, PilotRole, PilotStatus, Viewer,
    },
};
use crate::live::LiveFix;
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use serde::Serialize;

const MIGRATION_001: &str = include_str!("../../migrations/001_init.sql");

#[derive(Clone)]
pub struct ClubDb {
    #[allow(dead_code)]
    path: PathBuf,
    conn: Arc<Mutex<Connection>>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct PilotRow {
    #[allow(dead_code)]
    pub id: String,
    pub club_id: String,
    pub name: String,
    pub email: String,
    pub license_no: String,
    pub status: String,
    pub role: String,
}

#[derive(Clone, Debug)]
pub struct FlightRow {
    #[allow(dead_code)]
    pub id: String,
    pub pilot_id: String,
    #[allow(dead_code)]
    pub aircraft_id: String,
    #[allow(dead_code)]
    pub status: String,
    #[allow(dead_code)]
    pub started_at: i64,
    #[allow(dead_code)]
    pub ended_at: Option<i64>,
    #[allow(dead_code)]
    pub last_fix_at: Option<i64>,
    pub visibility: String,
    pub show_pilot_name_public: bool,
    pub tail: String,
    pub pilot_name: String,
}

impl ClubDb {
    pub fn open(data_dir: &Path) -> Result<Self, rusqlite::Error> {
        let path = data_dir.join("club.sqlite");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let db = Self {
            path,
            conn: Arc::new(Mutex::new(conn)),
        };
        db.migrate()?;
        db.bootstrap()?;
        Ok(db)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let db = Self {
            path: PathBuf::from(":memory:"),
            conn: Arc::new(Mutex::new(conn)),
        };
        db.migrate()?;
        db.bootstrap()?;
        Ok(db)
    }

    fn with_conn<F, T>(&self, f: F) -> Result<T, rusqlite::Error>
    where
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error>,
    {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&conn)
    }

    fn migrate(&self) -> Result<(), rusqlite::Error> {
        self.with_conn(|conn| {
            let applied: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .map(|n| n > 0)?;
            if applied {
                let version: Option<i64> = conn
                    .query_row(
                        "SELECT version FROM schema_migrations ORDER BY version DESC LIMIT 1",
                        [],
                        |r| r.get(0),
                    )
                    .optional()?;
                if version.is_some() {
                    return Ok(());
                }
            }
            conn.execute_batch(MIGRATION_001)?;
            conn.execute("INSERT OR IGNORE INTO schema_migrations (version) VALUES (1)", [])?;
            Ok(())
        })
    }

    fn bootstrap(&self) -> Result<(), rusqlite::Error> {
        self.with_conn(|conn| {
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM clubs", [], |r| r.get(0))?;
            if count == 0 {
                let now = now_ms();
                let club_id = new_id();
                conn.execute(
                    "INSERT INTO clubs (id, name, created_at) VALUES (?1, ?2, ?3)",
                    params![
                        club_id,
                        std::env::var("FLIGHT_CLUB_NAME")
                            .unwrap_or_else(|_| "Flight Club".to_string()),
                        now
                    ],
                )?;
            }
            if let (Ok(email), Ok(password)) = (
                std::env::var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_EMAIL"),
                std::env::var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_PASSWORD"),
            ) {
                if !email.is_empty() && !password.is_empty() {
                    let club_id: String =
                        conn.query_row("SELECT id FROM clubs LIMIT 1", [], |r| r.get(0))?;
                    let exists: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM pilots WHERE club_id = ?1 AND email = ?2",
                        params![club_id, email],
                        |r| r.get(0),
                    )?;
                    if exists == 0 {
                        let hash = auth::hash_password(&password).map_err(|_| {
                            rusqlite::Error::InvalidParameterName("hash".into())
                        })?;
                        conn.execute(
                            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
                             VALUES (?1, ?2, ?3, ?4, '', 'approved', 'admin', ?5, ?6)",
                            params![
                                new_id(),
                                club_id,
                                "Admin",
                                email,
                                hash,
                                now_ms()
                            ],
                        )?;
                    }
                }
            }
            Ok(())
        })
    }

    pub fn default_club_id(&self) -> Result<String, rusqlite::Error> {
        self.with_conn(|conn| conn.query_row("SELECT id FROM clubs LIMIT 1", [], |r| r.get(0)))
    }

    pub fn register(
        &self,
        club_id: &str,
        name: &str,
        email: &str,
        license_no: &str,
        password: &str,
    ) -> Result<String, RegisterError> {
        if password.len() < 8 {
            return Err(RegisterError::WeakPassword);
        }
        let hash = auth::hash_password(password).map_err(|_| RegisterError::Hash)?;
        let id = new_id();
        match self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending', 'pilot', ?6, ?7)",
                params![id, club_id, name, email, license_no, hash, now_ms()],
            )?;
            Ok(id)
        }) {
            Ok(id) => Ok(id),
            Err(e) => {
                if let rusqlite::Error::SqliteFailure(err, _) = &e {
                    if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE {
                        return Err(RegisterError::EmailExists);
                    }
                }
                Err(RegisterError::Db(e))
            }
        }
    }

    pub fn login(
        &self,
        club_id: &str,
        email: &str,
        password: &str,
        now: i64,
    ) -> Result<(String, PilotRow), LoginError> {
        let row = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT id, club_id, name, email, license_no, status, role, password_hash
                     FROM pilots WHERE club_id = ?1 AND email = ?2",
                    params![club_id, email],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, String>(4)?,
                            r.get::<_, String>(5)?,
                            r.get::<_, String>(6)?,
                            r.get::<_, String>(7)?,
                        ))
                    },
                )
                .optional()
            })
            .map_err(|_| LoginError::Invalid)?;
        let Some((
            id,
            club_id,
            name,
            email,
            license_no,
            status,
            role,
            hash,
        )) = row
        else {
            return Err(LoginError::Invalid);
        };
        if !auth::verify_password(password, &hash) {
            return Err(LoginError::Invalid);
        }
        let session_id = auth::new_session_id();
        let exp = now + SESSION_TTL_SEC;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO pilot_sessions (id, pilot_id, expires_at) VALUES (?1, ?2, ?3)",
                params![session_id, id, exp],
            )
        })
        .map_err(|_| LoginError::Invalid)?;
        Ok((
            session_id,
            PilotRow {
                id,
                club_id,
                name,
                email,
                license_no,
                status,
                role,
            },
        ))
    }

    pub fn logout(&self, session_id: &str) -> Result<(), rusqlite::Error> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM pilot_sessions WHERE id = ?1",
                params![session_id],
            )?;
            Ok(())
        })
    }

    pub fn pilot_from_session(&self, session_id: &str, now: i64) -> Option<PilotRow> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT p.id, p.club_id, p.name, p.email, p.license_no, p.status, p.role
                 FROM pilot_sessions s
                 JOIN pilots p ON p.id = s.pilot_id
                 WHERE s.id = ?1 AND s.expires_at > ?2",
                params![session_id, now],
                |r| {
                    Ok(PilotRow {
                        id: r.get(0)?,
                        club_id: r.get(1)?,
                        name: r.get(2)?,
                        email: r.get(3)?,
                        license_no: r.get(4)?,
                        status: r.get(5)?,
                        role: r.get(6)?,
                    })
                },
            )
            .optional()
        })
        .ok()
        .flatten()
    }

    pub fn viewer_from_session(
        &self,
        session_id: Option<&str>,
        now: i64,
    ) -> Viewer {
        let Some(session_id) = session_id else {
            return Viewer::Anonymous;
        };
        let Some(pilot) = self.pilot_from_session(session_id, now) else {
            return Viewer::Anonymous;
        };
        let status = match pilot.status.as_str() {
            "approved" => PilotStatus::Approved,
            _ => PilotStatus::Pending,
        };
        let role = match pilot.role.as_str() {
            "admin" => PilotRole::Admin,
            _ => PilotRole::Pilot,
        };
        Viewer::Pilot {
            id: pilot.id,
            status,
            role,
        }
    }

    pub fn viewer_with_pilot(&self, session_id: Option<&str>, now: i64) -> (Viewer, Option<PilotRow>) {
        let Some(session_id) = session_id else {
            return (Viewer::Anonymous, None);
        };
        let Some(pilot) = self.pilot_from_session(session_id, now) else {
            return (Viewer::Anonymous, None);
        };
        let viewer = self.viewer_from_session(Some(session_id), now);
        (viewer, Some(pilot))
    }

    pub fn approve_pilot(&self, pilot_id: &str) -> Result<(), rusqlite::Error> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE pilots SET status = 'approved' WHERE id = ?1",
                params![pilot_id],
            )?;
            Ok(())
        })
    }

    fn checkout_ok(
        conn: &Connection,
        pilot_id: &str,
        aircraft_id: &str,
        now: i64,
    ) -> Result<bool, rusqlite::Error> {
        let (type_id, active): (String, i64) = conn.query_row(
            "SELECT type_id, active FROM aircraft WHERE id = ?1",
            params![aircraft_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if active == 0 {
            return Ok(false);
        }
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM checkouts c
             WHERE c.pilot_id = ?1
               AND (c.expires_at IS NULL OR c.expires_at > ?2)
               AND (
                 (c.aircraft_id = ?3) OR
                 (c.aircraft_id IS NULL AND c.type_id = ?4)
               )",
            params![pilot_id, now, aircraft_id, type_id],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn me_tails(&self, pilot_id: &str, now: i64) -> Result<Vec<TailInfo>, rusqlite::Error> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT a.id, a.tail, t.code
                 FROM aircraft a
                 JOIN aircraft_types t ON t.id = a.type_id
                 WHERE a.active = 1",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, tail, type_code) = row?;
                if Self::checkout_ok(conn, pilot_id, &id, now)? {
                    out.push(TailInfo {
                        id,
                        tail,
                        type_code,
                    });
                }
            }
            Ok(out)
        })
    }

    pub fn start_flight(
        &self,
        pilot_id: &str,
        device_id: &str,
        tail: &str,
        visibility: FlightVisibility,
        show_pilot_name_public: bool,
        now: i64,
    ) -> Result<String, StartFlightError> {
        let aircraft_id = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT id FROM aircraft WHERE tail = ?1 AND active = 1",
                    params![tail],
                    |r| r.get::<_, String>(0),
                )
                .optional()
            })
            .map_err(|_| StartFlightError::NoTail)?;
        let Some(aircraft_id) = aircraft_id else {
            return Err(StartFlightError::NoTail);
        };
        let checkout = self
            .with_conn(|conn| Self::checkout_ok(conn, pilot_id, &aircraft_id, now))
            .unwrap_or(false);
        if !checkout {
            return Err(StartFlightError::NoCheckout);
        }
        let open = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM flights WHERE pilot_id = ?1 AND status = 'active'",
                    params![pilot_id],
                    |r| r.get::<_, i64>(0),
                )
            })
            .unwrap_or(0);
        if open > 0 {
            return Err(StartFlightError::AlreadyActive);
        }
        let flight_id = new_id();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO flights (id, pilot_id, aircraft_id, status, started_at, visibility, show_pilot_name_public)
                 VALUES (?1, ?2, ?3, 'active', ?4, ?5, ?6)",
                params![
                    flight_id,
                    pilot_id,
                    aircraft_id,
                    now,
                    visibility.as_str(),
                    i64::from(show_pilot_name_public)
                ],
            )?;
            conn.execute(
                "INSERT INTO flight_devices (flight_id, device_id) VALUES (?1, ?2)",
                params![flight_id, device_id],
            )?;
            Ok(())
        })
        .map_err(|_| StartFlightError::NoTail)?;
        Ok(flight_id)
    }

    pub fn patch_flight(
        &self,
        pilot_id: &str,
        flight_id: &str,
        visibility: Option<FlightVisibility>,
        show_pilot_name_public: Option<bool>,
    ) -> Result<(), PatchError> {
        let owner = match self.with_conn(|conn| {
            conn.query_row(
                "SELECT pilot_id FROM flights WHERE id = ?1",
                params![flight_id],
                |r| r.get::<_, String>(0),
            )
        }) {
            Ok(o) => o,
            Err(_) => return Err(PatchError::NotFound),
        };
        if owner != pilot_id {
            return Err(PatchError::Forbidden);
        }
        self.with_conn(|conn| {
            if let Some(v) = visibility {
                conn.execute(
                    "UPDATE flights SET visibility = ?1 WHERE id = ?2",
                    params![v.as_str(), flight_id],
                )?;
            }
            if let Some(flag) = show_pilot_name_public {
                conn.execute(
                    "UPDATE flights SET show_pilot_name_public = ?1 WHERE id = ?2",
                    params![i64::from(flag), flight_id],
                )?;
            }
            Ok(())
        })
        .map_err(|_| PatchError::NotFound)
    }

    pub fn end_flight(&self, pilot_id: &str, flight_id: &str, now: i64) -> Result<(), EndError> {
        let (owner, status) = match self.with_conn(|conn| {
            conn.query_row(
                "SELECT pilot_id, status FROM flights WHERE id = ?1",
                params![flight_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
        }) {
            Ok(row) => row,
            Err(_) => return Err(EndError::NotFound),
        };
        if owner != pilot_id {
            return Err(EndError::Forbidden);
        }
        if status != "active" {
            return Err(EndError::NotActive);
        }
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE flights SET status = 'ended', ended_at = ?1 WHERE id = ?2",
                params![now, flight_id],
            )
        })
        .map_err(|_| EndError::NotFound)?;
        Ok(())
    }

    pub fn resolve_device_token(&self, token: &str) -> Option<String> {
        let hash = digest_token(token);
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT id FROM devices WHERE token_hash = ?1",
                params![hash],
                |r| r.get(0),
            )
            .optional()
        })
        .ok()
        .flatten()
    }

    pub fn open_flight_for_device(&self, device_id: &str) -> Option<String> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT f.id FROM flights f
                 JOIN flight_devices fd ON fd.flight_id = f.id
                 WHERE fd.device_id = ?1 AND f.status = 'active'
                 ORDER BY f.started_at DESC LIMIT 1",
                params![device_id],
                |r| r.get(0),
            )
            .optional()
        })
        .ok()
        .flatten()
    }

    pub fn append_fix(&self, flight_id: &str, device_id: &str, fix: &LiveFix) -> Result<(), rusqlite::Error> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO fixes (flight_id, device_id, t_ms, lat, lon, alt, speed, course, accuracy)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    flight_id,
                    device_id,
                    fix.t,
                    fix.lat,
                    fix.lon,
                    fix.alt,
                    fix.speed,
                    fix.course,
                    fix.accuracy
                ],
            )?;
            conn.execute(
                "UPDATE flights SET last_fix_at = ?1 WHERE id = ?2",
                params![fix.t, flight_id],
            )?;
            Ok(())
        })
    }

    pub fn auto_end_idle(&self, idle_ms: i64, now: i64) -> Result<usize, rusqlite::Error> {
        self.with_conn(|conn| {
            let cutoff = now - idle_ms;
            let updated = conn.execute(
                "UPDATE flights SET status = 'ended', ended_at = ?1
                 WHERE status = 'active' AND (last_fix_at IS NULL AND started_at < ?2 OR last_fix_at < ?2)",
                params![now, cutoff],
            )?;
            Ok(updated)
        })
    }

    pub fn get_flight(&self, flight_id: &str) -> Option<FlightRow> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT f.id, f.pilot_id, f.aircraft_id, f.status, f.started_at, f.ended_at, f.last_fix_at,
                        f.visibility, f.show_pilot_name_public, a.tail, p.name
                 FROM flights f
                 JOIN aircraft a ON a.id = f.aircraft_id
                 JOIN pilots p ON p.id = f.pilot_id
                 WHERE f.id = ?1",
                params![flight_id],
                |r| {
                    Ok(FlightRow {
                        id: r.get(0)?,
                        pilot_id: r.get(1)?,
                        aircraft_id: r.get(2)?,
                        status: r.get(3)?,
                        started_at: r.get(4)?,
                        ended_at: r.get(5)?,
                        last_fix_at: r.get(6)?,
                        visibility: r.get(7)?,
                        show_pilot_name_public: r.get::<_, i64>(8)? != 0,
                        tail: r.get(9)?,
                        pilot_name: r.get(10)?,
                    })
                },
            )
            .optional()
        })
        .ok()
        .flatten()
    }

    pub fn flight_visible_to(&self, viewer: &Viewer, flight_id: &str) -> Option<FlightRow> {
        let flight = self.get_flight(flight_id)?;
        let vis = FlightVisibility::parse(&flight.visibility)?;
        if can_view_flight(viewer, &flight.pilot_id, vis) {
            Some(flight)
        } else {
            None
        }
    }

    pub fn fixes_for_flight(&self, flight_id: &str) -> Vec<LiveFix> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT t_ms, lat, lon, alt, speed, course, accuracy
                 FROM fixes WHERE flight_id = ?1 ORDER BY t_ms ASC",
            )?;
            let rows = stmt.query_map(params![flight_id], |r| {
                Ok(LiveFix {
                    t: r.get(0)?,
                    lat: r.get(1)?,
                    lon: r.get(2)?,
                    alt: r.get(3)?,
                    alt_ref: None,
                    speed: r.get(4)?,
                    course: r.get(5)?,
                    accuracy: r.get(6)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .unwrap_or_default()
    }

    fn latest_device_fixes(&self) -> Vec<DeviceFix> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT f.id, a.tail, f.pilot_id, p.name, f.visibility,
                        fx.t_ms, fx.lat, fx.lon, fx.alt, fx.speed, fx.course, fx.accuracy
                 FROM flights f
                 JOIN aircraft a ON a.id = f.aircraft_id
                 JOIN pilots p ON p.id = f.pilot_id
                 JOIN fixes fx ON fx.flight_id = f.id
                 WHERE f.status = 'active'
                   AND fx.t_ms = (
                     SELECT MAX(fx2.t_ms) FROM fixes fx2 WHERE fx2.flight_id = f.id
                   )",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(DeviceFix {
                    flight_id: r.get(0)?,
                    tail: r.get(1)?,
                    pilot_id: r.get(2)?,
                    pilot_name: r.get(3)?,
                    visibility: r.get(4)?,
                    fix: LiveFix {
                        t: r.get(5)?,
                        lat: r.get(6)?,
                        lon: r.get(7)?,
                        alt: r.get(8)?,
                        alt_ref: None,
                        speed: r.get(9)?,
                        course: r.get(10)?,
                        accuracy: r.get(11)?,
                    },
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .unwrap_or_default()
    }

    pub fn club_map_for_viewer(&self, viewer: &Viewer) -> Vec<TailLivePosition> {
        let rows = self.latest_device_fixes();
        let filtered: Vec<DeviceFix> = rows
            .into_iter()
            .filter(|r| {
                let vis = FlightVisibility::parse(&r.visibility).unwrap_or(FlightVisibility::Club);
                includes_in_club_map(viewer, &r.pilot_id, vis)
                    && can_view_flight(viewer, &r.pilot_id, vis)
            })
            .collect();
        let mut positions = dedupe_by_tail(filtered);
        for pos in &mut positions {
            if let Some(f) = self.get_flight(&pos.flight_id) {
                pos.pilot_name = Some(f.pilot_name);
            }
        }
        positions
    }

    pub fn public_map(&self) -> Vec<TailLivePosition> {
        let rows: Vec<DeviceFix> = self
            .latest_device_fixes()
            .into_iter()
            .filter(|r| {
                let vis =
                    FlightVisibility::parse(&r.visibility).unwrap_or(FlightVisibility::Club);
                includes_in_public_map(vis)
            })
            .collect();
        let mut positions = dedupe_by_tail(rows);
        for pos in &mut positions {
            let flight = self.get_flight(&pos.flight_id);
            if let Some(f) = flight {
                if show_pilot_name(&Viewer::Anonymous, &f.pilot_id, f.show_pilot_name_public) {
                    pos.pilot_name = Some(f.pilot_name);
                } else {
                    pos.pilot_name = None;
                }
            }
        }
        positions
    }

    pub fn admin_map(&self, admin_id: &str) -> Vec<TailLivePosition> {
        let viewer = Viewer::Pilot {
            id: admin_id.to_string(),
            status: PilotStatus::Approved,
            role: PilotRole::Admin,
        };
        let all = self.club_map_for_viewer(&viewer);
        let tails = self.admin_map_tails(admin_id);
        filter_admin_tails(all, &tails)
    }

    pub fn admin_map_tails(&self, admin_id: &str) -> Vec<String> {
        self.with_conn(|conn| {
            let json: String = conn
                .query_row(
                    "SELECT tails_json FROM admin_map_prefs WHERE pilot_id = ?1",
                    params![admin_id],
                    |r| r.get(0),
                )
                .unwrap_or_else(|_| "[]".to_string());
            Ok(serde_json::from_str(&json).unwrap_or_default())
        })
        .unwrap_or_default()
    }

    pub fn set_admin_map_tails(&self, admin_id: &str, tails: &[String]) -> Result<(), rusqlite::Error> {
        let json = serde_json::to_string(tails).unwrap_or_else(|_| "[]".to_string());
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO admin_map_prefs (pilot_id, tails_json) VALUES (?1, ?2)
                 ON CONFLICT(pilot_id) DO UPDATE SET tails_json = excluded.tails_json",
                params![admin_id, json],
            )?;
            Ok(())
        })
    }

    pub fn create_device(&self, pilot_id: &str, label: &str) -> Result<(String, String), rusqlite::Error> {
        let token = auth::new_device_token();
        let hash = digest_token(&token);
        let id = new_id();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO devices (id, pilot_id, token_hash, label, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, pilot_id, hash, label, now_ms()],
            )?;
            Ok((id, token))
        })
    }

    pub fn list_my_devices(&self, pilot_id: &str) -> Vec<DeviceInfo> {
        self.with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT id, label, created_at FROM devices WHERE pilot_id = ?1")?;
            let rows = stmt.query_map(params![pilot_id], |r| {
                Ok(DeviceInfo {
                    id: r.get(0)?,
                    label: r.get(1)?,
                    created_at: r.get(2)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .unwrap_or_default()
    }

    pub fn list_my_flights(&self, pilot_id: &str) -> Vec<FlightSummary> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT f.id, a.tail, f.status, f.started_at, f.ended_at, f.visibility, f.show_pilot_name_public
                 FROM flights f JOIN aircraft a ON a.id = f.aircraft_id
                 WHERE f.pilot_id = ?1 ORDER BY f.started_at DESC",
            )?;
            let rows = stmt.query_map(params![pilot_id], |r| {
                Ok(FlightSummary {
                    id: r.get(0)?,
                    tail: r.get(1)?,
                    status: r.get(2)?,
                    started_at: r.get(3)?,
                    ended_at: r.get(4)?,
                    visibility: r.get(5)?,
                    show_pilot_name_public: r.get::<_, i64>(6)? != 0,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .unwrap_or_default()
    }

    pub fn delete_my_flight(&self, pilot_id: &str, flight_id: &str) -> Result<(), DeleteError> {
        let owner: String = match self.with_conn(|conn| {
            conn.query_row(
                "SELECT pilot_id FROM flights WHERE id = ?1",
                params![flight_id],
                |r| r.get(0),
            )
        }) {
            Ok(o) => o,
            Err(_) => return Err(DeleteError::NotFound),
        };
        if owner != pilot_id {
            return Err(DeleteError::Forbidden);
        }
        self.with_conn(|conn| conn.execute("DELETE FROM flights WHERE id = ?1", params![flight_id]))
            .map_err(DeleteError::Db)?;
        Ok(())
    }

    // --- admin CRUD stubs ---
    pub fn upsert_type(&self, club_id: &str, code: &str) -> Result<String, rusqlite::Error> {
        self.with_conn(|conn| {
            if let Some(id) = conn
                .query_row(
                    "SELECT id FROM aircraft_types WHERE club_id = ?1 AND code = ?2",
                    params![club_id, code],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                return Ok(id);
            }
            let id = new_id();
            conn.execute(
                "INSERT INTO aircraft_types (id, club_id, code) VALUES (?1, ?2, ?3)",
                params![id, club_id, code],
            )?;
            Ok(id)
        })
    }

    pub fn upsert_aircraft(
        &self,
        club_id: &str,
        tail: &str,
        type_code: &str,
        active: bool,
    ) -> Result<String, rusqlite::Error> {
        self.with_conn(|conn| {
            let type_id: String = conn.query_row(
                "SELECT id FROM aircraft_types WHERE club_id = ?1 AND code = ?2",
                params![club_id, type_code],
                |r| r.get(0),
            )?;
            if let Some(id) = conn
                .query_row(
                    "SELECT id FROM aircraft WHERE club_id = ?1 AND tail = ?2",
                    params![club_id, tail],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                conn.execute(
                    "UPDATE aircraft SET type_id = ?1, active = ?2 WHERE id = ?3",
                    params![type_id, i64::from(active), id],
                )?;
                return Ok(id);
            }
            let id = new_id();
            conn.execute(
                "INSERT INTO aircraft (id, club_id, tail, type_id, active) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, club_id, tail, type_id, i64::from(active)],
            )?;
            Ok(id)
        })
    }

    pub fn add_checkout(
        &self,
        pilot_id: &str,
        type_code: &str,
        tail: Option<&str>,
        expires_at: Option<i64>,
    ) -> Result<(), rusqlite::Error> {
        self.with_conn(|conn| {
            let club_id: String = conn.query_row(
                "SELECT club_id FROM pilots WHERE id = ?1",
                params![pilot_id],
                |r| r.get(0),
            )?;
            let type_id: String = conn.query_row(
                "SELECT id FROM aircraft_types WHERE club_id = ?1 AND code = ?2",
                params![club_id, type_code],
                |r| r.get(0),
            )?;
            let aircraft_id: Option<String> = if let Some(tail) = tail {
                conn.query_row(
                    "SELECT id FROM aircraft WHERE club_id = ?1 AND tail = ?2",
                    params![club_id, tail],
                    |r| r.get(0),
                )
                .optional()?
            } else {
                None
            };
            conn.execute(
                "INSERT INTO checkouts (id, pilot_id, type_id, aircraft_id, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![new_id(), pilot_id, type_id, aircraft_id, expires_at],
            )?;
            Ok(())
        })
    }

    pub fn list_pending_pilots(&self) -> Vec<PilotRow> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, club_id, name, email, license_no, status, role
                 FROM pilots WHERE status = 'pending'",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(PilotRow {
                    id: r.get(0)?,
                    club_id: r.get(1)?,
                    name: r.get(2)?,
                    email: r.get(3)?,
                    license_no: r.get(4)?,
                    status: r.get(5)?,
                    role: r.get(6)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .unwrap_or_default()
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug)]
pub enum RegisterError {
    WeakPassword,
    EmailExists,
    Hash,
    Db(#[allow(dead_code)] rusqlite::Error),
}

#[derive(Debug)]
pub enum LoginError {
    Invalid,
}

#[derive(Debug)]
pub enum StartFlightError {
    NoTail,
    NoCheckout,
    AlreadyActive,
}

#[derive(Debug)]
pub enum PatchError {
    NotFound,
    Forbidden,
}

#[derive(Debug)]
pub enum EndError {
    NotFound,
    Forbidden,
    NotActive,
}

#[derive(Debug)]
pub enum DeleteError {
    NotFound,
    Forbidden,
    Db(#[allow(dead_code)] rusqlite::Error),
}

#[derive(Clone, Serialize)]
pub struct TailInfo {
    pub id: String,
    pub tail: String,
    pub type_code: String,
}

#[derive(Clone, Serialize)]
pub struct DeviceInfo {
    pub id: String,
    pub label: String,
    pub created_at: i64,
}

#[derive(Clone, Serialize)]
pub struct FlightSummary {
    pub id: String,
    pub tail: String,
    pub status: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub visibility: String,
    pub show_pilot_name_public: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::club::visibility::{FlightVisibility, PilotRole, PilotStatus, Viewer};

    fn seed(db: &ClubDb) -> (String, String, String, String) {
        let club = db.default_club_id().unwrap();
        let type_id = db.upsert_type(&club, "C152").unwrap();
        let _ = type_id;
        let ac = db.upsert_aircraft(&club, "N123", "C152", true).unwrap();
        let pilot_a = db
            .register(&club, "Alice", "a@test", "L1", "password12")
            .unwrap();
        db.approve_pilot(&pilot_a).unwrap();
        let pilot_b = db
            .register(&club, "Bob", "b@test", "L2", "password12")
            .unwrap();
        db.approve_pilot(&pilot_b).unwrap();
        db.add_checkout(&pilot_a, "C152", None, None).unwrap();
        db.add_checkout(&pilot_b, "C152", None, None).unwrap();
        (club, pilot_a, pilot_b, ac)
    }

    #[test]
    fn wrong_type_checkout_blocks_tail() {
        let db = ClubDb::open_in_memory().unwrap();
        let club = db.default_club_id().unwrap();
        db.upsert_type(&club, "DA20").unwrap();
        db.upsert_type(&club, "C152").unwrap();
        db.upsert_aircraft(&club, "N1", "C152", true).unwrap();
        let a = db
            .register(&club, "A", "a@test", "", "password12")
            .unwrap();
        db.approve_pilot(&a).unwrap();
        db.add_checkout(&a, "DA20", None, None).unwrap();
        let tails = db.me_tails(&a, now_ms()).unwrap();
        assert!(tails.is_empty());
    }

    #[test]
    fn checkout_expired_blocks_start() {
        let db = ClubDb::open_in_memory().unwrap();
        let club = db.default_club_id().unwrap();
        db.upsert_type(&club, "C152").unwrap();
        db.upsert_aircraft(&club, "N123", "C152", true).unwrap();
        let a = db
            .register(&club, "A", "a@test", "", "password12")
            .unwrap();
        db.approve_pilot(&a).unwrap();
        let expired = now_ms() - 1000;
        db.add_checkout(&a, "C152", None, Some(expired))
            .unwrap();
        let tails = db.me_tails(&a, now_ms()).unwrap();
        assert!(tails.is_empty());
    }

    #[test]
    fn checkout_blocks_inactive_tail() {
        let db = ClubDb::open_in_memory().unwrap();
        let club = db.default_club_id().unwrap();
        db.upsert_type(&club, "DA20").unwrap();
        db.upsert_aircraft(&club, "N999", "DA20", false).unwrap();
        let p = db
            .register(&club, "P", "p@test", "", "password12")
            .unwrap();
        db.approve_pilot(&p).unwrap();
        db.add_checkout(&p, "DA20", None, None).unwrap();
        let tails = db.me_tails(&p, now_ms()).unwrap();
        assert!(tails.is_empty());
    }

    #[test]
    fn visibility_private_hidden_on_club_map() {
        let db = ClubDb::open_in_memory().unwrap();
        let (_club, a, b, _ac) = seed(&db);
        let dev = db.create_device(&a, "phone").unwrap().0;
        let fid = db
            .start_flight(
                &a,
                &dev,
                "N123",
                FlightVisibility::Private,
                false,
                now_ms(),
            )
            .unwrap();
        let fix = LiveFix {
            t: now_ms(),
            lat: -33.4,
            lon: -70.6,
            alt: Some(1.0),
            alt_ref: None,
            speed: None,
            course: None,
            accuracy: Some(5.0),
        };
        db.append_fix(&fid, &dev, &fix).unwrap();
        let viewer_b = Viewer::Pilot {
            id: b.clone(),
            status: PilotStatus::Approved,
            role: PilotRole::Pilot,
        };
        let map = db.club_map_for_viewer(&viewer_b);
        assert!(map.is_empty());
        let viewer_a = Viewer::Pilot {
            id: a.clone(),
            status: PilotStatus::Approved,
            role: PilotRole::Pilot,
        };
        assert_eq!(db.club_map_for_viewer(&viewer_a).len(), 1);
    }

    #[test]
    fn admin_tail_filter_does_not_change_pilot_map() {
        let db = ClubDb::open_in_memory().unwrap();
        let (club, a, _b, _ac) = seed(&db);
        db.upsert_aircraft(&club, "N456", "C152", true).unwrap();
        db.add_checkout(&a, "C152", Some("N456"), None).unwrap();
        let dev = db.create_device(&a, "phone").unwrap().0;
        let f1 = db
            .start_flight(&a, &dev, "N123", FlightVisibility::Club, false, now_ms())
            .unwrap();
        db.end_flight(&a, &f1, now_ms()).unwrap();
        let f2 = db
            .start_flight(&a, &dev, "N456", FlightVisibility::Club, false, now_ms() + 1)
            .unwrap();
        let fix = LiveFix {
            t: now_ms(),
            lat: -33.4,
            lon: -70.6,
            alt: None,
            alt_ref: None,
            speed: None,
            course: None,
            accuracy: Some(5.0),
        };
        db.append_fix(&f2, &dev, &fix).unwrap();
        db.set_admin_map_tails(&a, &["N123".to_string()]).unwrap();
        let admin_map = db.admin_map(&a);
        assert!(admin_map.is_empty());
        let pilot_map = db.club_map_for_viewer(&Viewer::Pilot {
            id: a.clone(),
            status: PilotStatus::Approved,
            role: PilotRole::Pilot,
        });
        assert_eq!(pilot_map.len(), 1);
        assert_eq!(pilot_map[0].tail, "N456");
    }

    #[test]
    fn pending_pilot_club_map_empty() {
        let db = ClubDb::open_in_memory().unwrap();
        let (_club, _a, _b, _ac) = seed(&db);
        let pending = db.register(&db.default_club_id().unwrap(), "P", "p2@test", "", "password12").unwrap();
        let viewer = Viewer::Pilot {
            id: pending,
            status: PilotStatus::Pending,
            role: PilotRole::Pilot,
        };
        assert!(db.club_map_for_viewer(&viewer).is_empty());
    }

    #[test]
    fn public_map_anonymous_no_name_without_flag() {
        let db = ClubDb::open_in_memory().unwrap();
        let (_club, a, _b, _ac) = seed(&db);
        let dev = db.create_device(&a, "phone").unwrap().0;
        let fid = db
            .start_flight(
                &a,
                &dev,
                "N123",
                FlightVisibility::Public,
                false,
                now_ms(),
            )
            .unwrap();
        db.append_fix(
            &fid,
            &dev,
            &LiveFix {
                t: now_ms(),
                lat: 1.0,
                lon: 2.0,
                alt: None,
                alt_ref: None,
                speed: None,
                course: None,
                accuracy: Some(5.0),
            },
        )
        .unwrap();
        let map = db.public_map();
        assert_eq!(map.len(), 1);
        assert!(map[0].pilot_name.is_none());
    }

    #[test]
    fn mid_flight_visibility_change_affects_access() {
        let db = ClubDb::open_in_memory().unwrap();
        let (_club, a, b, _ac) = seed(&db);
        let dev = db.create_device(&a, "phone").unwrap().0;
        let fid = db
            .start_flight(&a, &dev, "N123", FlightVisibility::Club, false, now_ms())
            .unwrap();
        let viewer_b = Viewer::Pilot {
            id: b.clone(),
            status: PilotStatus::Approved,
            role: PilotRole::Pilot,
        };
        assert!(db.flight_visible_to(&viewer_b, &fid).is_some());
        db.patch_flight(&a, &fid, Some(FlightVisibility::Private), None)
            .unwrap();
        assert!(db.flight_visible_to(&viewer_b, &fid).is_none());
    }
}