use crate::club::{
    auth::{self, digest_token, new_device_token, new_id, SESSION_TTL_SEC},
    checkin::{self, CheckinTokenError},
    database::{self, DbConnectError},
    map::{dedupe_by_tail, filter_admin_tails, DeviceFix, TailLivePosition},
    visibility::{
        can_view_flight, includes_in_club_map, includes_in_public_map, show_pilot_name,
        FlightVisibility, PilotRole, PilotStatus, Viewer,
    },
};
use crate::live::LiveFix;
use serde::Serialize;
use sqlx::{PgPool, Row};
use std::borrow::Cow;

pub type DbError = sqlx::Error;

#[derive(Clone)]
pub struct ClubDb {
    pub(crate) pool: PgPool,
}

pub const MAX_DEVICES_PER_PILOT: usize = 10;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceRegisterError { InvalidInstallId, CapExceeded, Db, }
pub enum ShareCreateError { Db(DbError), NotFound, NotOwner, }
pub enum ShareRevokeError { Db(DbError), NotFound, NotOwner, }
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ApprovePilotCheckout { #[serde(rename="type")] pub type_code: String, pub tail: Option<String>, pub expires_at: Option<i64>, }
#[derive(serde::Serialize)]
pub struct AdminPendingCounts { pub pilots: usize, pub access_requests: usize, }

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
    // from db.rs
pub async fn connect() -> Result<Self, DbConnectError> {
        let pool = database::connect_pool().await?;
        database::run_migrations(&pool).await?;
        let db = Self { pool };
        db.bootstrap().await?;
        Ok(db)
    }

    #[cfg(test)]
    pub async fn open_test() -> Result<Self, DbConnectError> {
        let pool = database::open_test_pool().await?;
        let db = Self { pool };
        db.truncate_all().await.map_err(DbConnectError::from)?;
        db.bootstrap().await?;
        Ok(db)
    }

    async fn truncate_all(&self) -> Result<(), DbError> {
        sqlx::query(
            "TRUNCATE TABLE audit_log, access_requests, checkins, device_pair_tokens, flight_devices, fixes, flights, \
             devices, checkouts, pilot_sessions, admin_map_prefs, display_sessions, \
             pilots, aircraft, aircraft_types, clubs RESTART IDENTITY CASCADE",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }


    async fn bootstrap(&self) -> Result<(), DbError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM clubs")
            .fetch_one(&self.pool)
            .await?;
        if count == 0 {
            let now = now_ms();
            let club_id = new_id();
            sqlx::query("INSERT INTO clubs (id, name, created_at) VALUES ($1, $2, $3)")
                .bind(club_id)
                .bind(
                    std::env::var("FLIGHT_CLUB_NAME")
                        .unwrap_or_else(|_| "Flight Club".to_string()),
                )
                .bind(now)
                .execute(&self.pool)
                .await?;
        }
        if let (Ok(email), Ok(password)) = (
            std::env::var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_EMAIL"),
            std::env::var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_PASSWORD"),
        ) {
            if !email.is_empty() && !password.is_empty() {
                let club_id: String = sqlx::query_scalar("SELECT id FROM clubs LIMIT 1")
                    .fetch_one(&self.pool)
                    .await?;
                let exists: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*)::bigint FROM pilots WHERE club_id = $1 AND email = $2",
                )
                .bind(&club_id)
                .bind(&email)
                .fetch_one(&self.pool)
                .await?;
                if exists == 0 {
                    let hash = auth::hash_password(&password)
                        .map_err(|_| DbError::InvalidParameterName("hash".into()))?;
                    sqlx::query(
                        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
                         VALUES ($1, $2, $3, $4, '', 'approved', 'admin', $5, $6)",
                    )
                    .bind(new_id())
                    .bind(club_id)
                    .bind("Admin")
                    .bind(email)
                    .bind(hash)
                    .bind(now_ms())
                    .execute(&self.pool)
                    .await?;
                }
            }
        }
        Ok(())
    }

    pub async fn default_club_id(&self) -> Result<String, DbError> {
        sqlx::query_scalar("SELECT id FROM clubs LIMIT 1")
            .fetch_one(&self.pool)
            .await
    }

    pub async fn register(
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
        match sqlx::query(
            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
             VALUES ($1, $2, $3, $4, $5, 'pending', 'pilot', $6, $7)",
        )
        .bind(&id)
        .bind(club_id)
        .bind(name)
        .bind(email)
        .bind(license_no)
        .bind(hash)
        .bind(now_ms())
        .execute(&self.pool)
        .await
        {
            Ok(_) => Ok(id),
            Err(e) => {
                if let Some(db_err) = e.as_database_error() {
                    if db_err.code() == Some(Cow::Borrowed("23505")) {
                        return Err(RegisterError::EmailExists);
                    }
                }
                Err(RegisterError::Db(e))
            }
        }
    }

    pub async fn login(
        &self,
        club_id: &str,
        email: &str,
        password: &str,
        now: i64,
    ) -> Result<(String, PilotRow), LoginError> {
        let row = sqlx::query(
            "SELECT id, club_id, name, email, license_no, status, role, password_hash
             FROM pilots WHERE club_id = $1 AND email = $2",
        )
        .bind(club_id)
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| LoginError::Invalid)?
        .map(|r| -> Result<_, DbError> {
            Ok((
                r.try_get(0)?,
                r.try_get(1)?,
                r.try_get(2)?,
                r.try_get(3)?,
                r.try_get(4)?,
                r.try_get(5)?,
                r.try_get(6)?,
                r.try_get(7)?,
            ))
        })
        .transpose()
        .map_err(|_| LoginError::Invalid)?;
        let Some((id, club_id, name, email, license_no, status, role, hash)) = row else {
            return Err(LoginError::Invalid);
        };
        if !auth::verify_password(password, &hash) {
            return Err(LoginError::Invalid);
        }
        let session_id = auth::new_session_id();
        let exp = now + SESSION_TTL_SEC * 1000;
        sqlx::query("INSERT INTO pilot_sessions (id, pilot_id, expires_at) VALUES ($1, $2, $3)")
            .bind(&session_id)
            .bind(&id)
            .bind(exp)
            .execute(&self.pool)
            .await
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

    pub async fn logout(&self, session_id: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM pilot_sessions WHERE id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn pilot_from_session(&self, session_id: &str, now: i64) -> Option<PilotRow> {
        sqlx::query(
            "SELECT p.id, p.club_id, p.name, p.email, p.license_no, p.status, p.role
             FROM pilot_sessions s
             JOIN pilots p ON p.id = s.pilot_id
             WHERE s.id = $1 AND s.expires_at > $2",
        )
        .bind(session_id)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .map(|row| PilotRow {
            id: row.try_get(0).ok()?,
            club_id: row.try_get(1).ok()?,
            name: row.try_get(2).ok()?,
            email: row.try_get(3).ok()?,
            license_no: row.try_get(4).ok()?,
            status: row.try_get(5).ok()?,
            role: row.try_get(6).ok()?,
        })
    }

    pub async fn viewer_from_session(&self, session_id: Option<&str>, now: i64) -> Viewer {
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

    pub async fn viewer_with_pilot(
        &self,
        session_id: Option<&str>,
        now: i64,
    ) -> (Viewer, Option<PilotRow>) {
        let Some(session_id) = session_id else {
            return (Viewer::Anonymous, None);
        };
        let Some(pilot) = self.pilot_from_session(session_id, now) else {
            return (Viewer::Anonymous, None);
        };
        let viewer = self.viewer_from_session(Some(session_id), now);
        (viewer, Some(pilot))
    }

    pub async fn approve_pilot(&self, pilot_id: &str) -> Result<(), DbError> {
        
            sqlx::query("UPDATE pilots SET status = 'approved' WHERE id = $1").bind(pilot_id).execute(&self.pool).await?;
            Ok(())
        
    }

    async fn checkout_ok(
        pool: &PgPool,
        pilot_id: &str,
        aircraft_id: &str,
        now: i64,
    ) -> Result<bool, DbError> {
        let row = sqlx::query("SELECT type_id, active FROM aircraft WHERE id = $1")
            .bind(aircraft_id)
            .fetch_one(pool)
            .await?;
        let type_id: String = row.try_get(0)?;
        let active: bool = row.try_get(1)?;
        if !active {
            return Ok(false);
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM checkouts c
             WHERE c.pilot_id = $1
               AND (c.expires_at IS NULL OR c.expires_at > $2)
               AND ((c.aircraft_id = $3) OR (c.aircraft_id IS NULL AND c.type_id = $4))",
        )
        .bind(pilot_id)
        .bind(now)
        .bind(aircraft_id)
        .bind(&type_id)
        .fetch_one(pool)
        .await?;
        Ok(count > 0)
    }

    pub async fn me_tails(&self, pilot_id: &str, now: i64) -> Result<Vec<TailInfo>, DbError> {
        let rows = sqlx::query(
            "SELECT a.id, a.tail, t.code FROM aircraft a
             JOIN aircraft_types t ON t.id = a.type_id
             WHERE a.active = TRUE",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for row in rows {
            let id: String = row.try_get(0)?;
            let tail: String = row.try_get(1)?;
            let type_code: String = row.try_get(2)?;
            if Self::checkout_ok(&self.pool, pilot_id, &id, now).await? {
                out.push(TailInfo { id, tail, type_code });
            }
        }
        Ok(out)
    }

    pub async fn start_flight(
        &self,
        pilot_id: &str,
        device_id: &str,
        tail: &str,
        visibility: FlightVisibility,
        show_pilot_name_public: bool,
        now: i64,
    ) -> Result<String, StartFlightError> {
        let aircraft_id = sqlx::query_scalar::<_, String>(
            "SELECT id FROM aircraft WHERE tail = $1 AND active = TRUE",
        )
        .bind(tail)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| StartFlightError::NoTail)?;
        let Some(aircraft_id) = aircraft_id else {
            return Err(StartFlightError::NoTail);
        };
        let type_code: String = sqlx::query_scalar(
            "SELECT t.code FROM aircraft a JOIN aircraft_types t ON t.id = a.type_id WHERE a.id = $1",
        )
        .bind(&aircraft_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| StartFlightError::NoTail)?;
        let checkout = Self::checkout_ok(&self.pool, pilot_id, &aircraft_id, now)
            .await
            .unwrap_or(false);
        if !checkout {
            return Err(StartFlightError::NotCleared {
                tail: tail.to_string(),
                type_code,
            });
        }
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM flights WHERE pilot_id = $1 AND status = 'active'",
        )
        .bind(pilot_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        if open > 0 {
            return Err(StartFlightError::AlreadyActive);
        }
        let club_id: String = sqlx::query_scalar("SELECT club_id FROM aircraft WHERE id = $1")
            .bind(&aircraft_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| StartFlightError::NoTail)?;
        if self.device_pilot_id(device_id).await.as_deref() != Some(pilot_id) {
            return Err(StartFlightError::DeviceMismatch);
        }
        if !self
            .consume_checkin(pilot_id, device_id, &club_id, now)
            .await
            .map_err(|_| StartFlightError::NoCheckin)?
        {
            return Err(StartFlightError::NoCheckin);
        }
        let flight_id = new_id();
        sqlx::query(
            "INSERT INTO flights (id, pilot_id, aircraft_id, status, started_at, visibility, show_pilot_name_public)
             VALUES ($1, $2, $3, 'active', $4, $5, $6)",
        )
        .bind(&flight_id)
        .bind(pilot_id)
        .bind(&aircraft_id)
        .bind(now)
        .bind(visibility.as_str())
        .bind(show_pilot_name_public)
        .execute(&self.pool)
        .await
        .map_err(|_| StartFlightError::NoTail)?;
        sqlx::query("INSERT INTO flight_devices (flight_id, device_id) VALUES ($1, $2)")
            .bind(&flight_id)
            .bind(device_id)
            .execute(&self.pool)
            .await
            .map_err(|_| StartFlightError::NoTail)?;
        Ok(flight_id)
    }

    pub async fn patch_flight(
        &self,
        pilot_id: &str,
        flight_id: &str,
        visibility: Option<FlightVisibility>,
        show_pilot_name_public: Option<bool>,
    ) -> Result<(), PatchError> {
        let owner: String = sqlx::query_scalar("SELECT pilot_id FROM flights WHERE id = $1")
            .bind(flight_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| PatchError::NotFound)?;
        if owner != pilot_id {
            return Err(PatchError::Forbidden);
        }
        if let Some(v) = visibility {
            sqlx::query("UPDATE flights SET visibility = $1 WHERE id = $2")
                .bind(v.as_str())
                .bind(flight_id)
                .execute(&self.pool)
                .await
                .map_err(|_| PatchError::NotFound)?;
        }
        if let Some(flag) = show_pilot_name_public {
            sqlx::query("UPDATE flights SET show_pilot_name_public = $1 WHERE id = $2")
                .bind(flag)
                .bind(flight_id)
                .execute(&self.pool)
                .await
                .map_err(|_| PatchError::NotFound)?;
        }
        Ok(())
    }

    pub async fn end_flight(&self, pilot_id: &str, flight_id: &str, now: i64) -> Result<(), EndError> {
        let row = sqlx::query("SELECT pilot_id, status FROM flights WHERE id = $1")
            .bind(flight_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| EndError::NotFound)?;
        let owner: String = row.try_get(0).map_err(|_| EndError::NotFound)?;
        let status: String = row.try_get(1).map_err(|_| EndError::NotFound)?;
        if owner != pilot_id {
            return Err(EndError::Forbidden);
        }
        if status != "active" {
            return Err(EndError::NotActive);
        }
        sqlx::query("UPDATE flights SET status = 'ended', ended_at = $1 WHERE id = $2")
            .bind(now)
            .bind(flight_id)
            .execute(&self.pool)
            .await
            .map_err(|_| EndError::NotFound)?;
        Ok(())
    }

    pub async fn device_pilot_id(&self, device_id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT pilot_id FROM devices WHERE id = $1")
            .bind(device_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    pub async fn resolve_device_token(&self, token: &str) -> Option<String> {
        let hash = digest_token(token);
        sqlx::query_scalar("SELECT device_id FROM devices WHERE token_hash = $1")
            .bind(hash)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    pub async fn open_flight_for_device(&self, device_id: &str) -> Option<String> {
        sqlx::query_scalar(
            "SELECT f.id FROM flights f
             JOIN flight_devices fd ON fd.flight_id = f.id
             WHERE fd.device_id = $1 AND f.status = 'active'
             ORDER BY f.started_at DESC LIMIT 1",
        )
        .bind(device_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    pub async fn append_fix(
        &self,
        flight_id: &str,
        device_id: &str,
        fix: &LiveFix,
    ) -> Result<(), DbError> {
        self.append_fixes(flight_id, device_id, std::slice::from_ref(fix))
            .map(|_| ())
    }

    pub fn append_fixes(
        &self,
        flight_id: &str,
        device_id: &str,
        fixes: &[LiveFix],
    ) -> Result<usize, DbError> {
        if fixes.is_empty() {
            return Ok(0);
        }
        
            let mut tx = self.pool.begin().await?;
            let mut inserted = 0usize;
            let mut last_t = fixes[0].t;
            for fix in fixes {
                let changed = sqlx::query("INSERT OR IGNORE INTO fixes (flight_id, device_id, t_ms, lat, lon, alt, speed, course, accuracy)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)").bind(flight_id).bind(device_id).bind(fix.t).bind(fix.lat).bind(fix.lon).bind(fix.alt).bind(fix.speed).bind(fix.course).bind(fix.accuracy).execute(&self.pool).await?;
                inserted += changed;
                last_t = fix.t;
            }
            sqlx::query("UPDATE flights SET last_fix_at = $1 WHERE id = $2").bind(last_t).bind(flight_id).execute(&self.pool).await?;
            tx.commit().await?;
            Ok(inserted)
        
    }

    pub async fn auto_end_idle(&self, idle_ms: i64, now: i64) -> Result<usize, DbError> {
        
            let cutoff = now - idle_ms;
            let updated = sqlx::query("UPDATE flights SET status = 'ended', ended_at = $1
                 WHERE status = 'active' AND (last_fix_at IS NULL AND started_at < $2 OR last_fix_at < $2)").bind(now).bind(cutoff).execute(&self.pool).await?;
            Ok(updated)
        
    }

    pub fn get_flight(&self, flight_id: &str) -> Option<FlightRow> {
        
            conn.query_row(
                "SELECT f.id, f.pilot_id, f.aircraft_id, f.status, f.started_at, f.ended_at, f.last_fix_at,
                        f.visibility, f.show_pilot_name_public, a.tail, p.name
                 FROM flights f
                 JOIN aircraft a ON a.id = f.aircraft_id
                 JOIN pilots p ON p.id = f.pilot_id
                 WHERE f.id = $1",
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
        
        .ok()
        .flatten()
    }

    pub async fn flight_visible_to(&self, viewer: &Viewer, flight_id: &str) -> Option<FlightRow> {
        let flight = self.get_flight(flight_id)?;
        let vis = FlightVisibility::parse(&flight.visibility)?;
        if can_view_flight(viewer, &flight.pilot_id, vis) {
            Some(flight)
        } else {
            None
        }
    }

    pub fn fixes_for_flight(&self, flight_id: &str) -> Vec<LiveFix> {
        
            let mut stmt = conn.prepare(
                "SELECT t_ms, lat, lon, alt, speed, course, accuracy
                 FROM fixes WHERE flight_id = $1 ORDER BY t_ms ASC",
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
        
        .unwrap_or_default()
    }

    fn latest_device_fixes(&self) -> Vec<DeviceFix> {
        
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
        
        .unwrap_or_default()
    }

    pub async fn club_map_for_viewer(&self, viewer: &Viewer) -> Vec<TailLivePosition> {
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

    pub async fn public_map(&self) -> Vec<TailLivePosition> {
        let rows: Vec<DeviceFix> = self
            .latest_device_fixes()
            .into_iter()
            .filter(|r| {
                let vis = FlightVisibility::parse(&r.visibility).unwrap_or(FlightVisibility::Club);
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

    pub async fn admin_map(&self, admin_id: &str) -> Vec<TailLivePosition> {
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
        
            let json: String = conn
                .query_row(
                    "SELECT tails_json FROM admin_map_prefs WHERE pilot_id = $1",
                    params![admin_id],
                    |r| r.get(0),
                )
                .unwrap_or_else(|_| "[]".to_string());
            Ok(serde_json::from_str(&json).unwrap_or_default())
        
        .unwrap_or_default()
    }

    pub async fn set_admin_map_tails(
        &self,
        admin_id: &str,
        tails: &[String],
    ) -> Result<(), DbError> {
        let json = serde_json::to_string(tails).unwrap_or_else(|_| "[]".to_string());
        
            sqlx::query("INSERT INTO admin_map_prefs (pilot_id, tails_json) VALUES ($1, $2)
                 ON CONFLICT(pilot_id) DO UPDATE SET tails_json = excluded.tails_json").bind(admin_id).bind(json).execute(&self.pool).await?;
            Ok(())
        
    }

    pub async fn create_device(
        &self,
        pilot_id: &str,
        label: &str,
    ) -> Result<(String, String), DbError> {
        let token = auth::new_device_token();
        let hash = digest_token(&token);
        let id = new_id();
        
            sqlx::query("INSERT INTO devices (id, pilot_id, token_hash, label, created_at) VALUES ($1, $2, $3, $4, $5)").bind(id).bind(pilot_id).bind(hash).bind(label).bind(now_ms()).execute(&self.pool).await?;
            sqlx::query("INSERT INTO device_pair_tokens (device_id, token) VALUES ($1, $2)").bind(id).bind(token).execute(&self.pool).await?;
            Ok((id, token))
        
    }

    pub fn device_pair_token_for_pilot(&self, device_id: &str, pilot_id: &str) -> Option<String> {
        
            conn.query_row(
                "SELECT dpt.token FROM device_pair_tokens dpt
                 JOIN devices d ON d.id = dpt.device_id
                 WHERE dpt.device_id = $1 AND d.pilot_id = $2",
                params![device_id, pilot_id],
                |r| r.get(0),
            )
            .optional()
        
        .ok()
        .flatten()
    }

    pub fn list_my_devices(&self, pilot_id: &str) -> Vec<DeviceInfo> {
        
            let mut stmt =
                conn.prepare("SELECT id, label, created_at FROM devices WHERE pilot_id = $1")?;
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
        
        .unwrap_or_default()
    }

    pub fn list_my_flights(&self, pilot_id: &str) -> Vec<FlightSummary> {
        
            let mut stmt = conn.prepare(
                "SELECT f.id, a.tail, f.status, f.started_at, f.ended_at, f.visibility, f.show_pilot_name_public
                 FROM flights f JOIN aircraft a ON a.id = f.aircraft_id
                 WHERE f.pilot_id = $1 ORDER BY f.started_at DESC",
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
        
        .unwrap_or_default()
    }

    pub fn delete_my_flight(&self, pilot_id: &str, flight_id: &str) -> Result<(), DeleteError> {
        let owner: String = match 
            conn.query_row(
                "SELECT pilot_id FROM flights WHERE id = $1",
                params![flight_id],
                |r| r.get(0),
            )
         {
            Ok(o) => o,
            Err(_) => return Err(DeleteError::NotFound),
        };
        if owner != pilot_id {
            return Err(DeleteError::Forbidden);
        }
        
            sqlx::query("DELETE FROM flights WHERE id = $1").bind(flight_id).execute(&self.pool).await
        
        .map_err(DeleteError::Db)?;
        Ok(())
    }

    pub async fn upsert_type(&self, club_id: &str, code: &str) -> Result<String, DbError> {
        
            if let Some(id) = conn
                .query_row(
                    "SELECT id FROM aircraft_types WHERE club_id = $1 AND code = $2",
                    params![club_id, code],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                return Ok(id);
            }
            let id = new_id();
            sqlx::query("INSERT INTO aircraft_types (id, club_id, code) VALUES ($1, $2, $3)").bind(id).bind(club_id).bind(code).execute(&self.pool).await?;
            Ok(id)
        
    }

    pub fn upsert_aircraft(
        &self,
        club_id: &str,
        tail: &str,
        type_code: &str,
        active: bool,
    ) -> Result<String, DbError> {
        
            let type_id: String = conn.query_row(
                "SELECT id FROM aircraft_types WHERE club_id = $1 AND code = $2",
                params![club_id, type_code],
                |r| r.get(0),
            )?;
            if let Some(id) = conn
                .query_row(
                    "SELECT id FROM aircraft WHERE club_id = $1 AND tail = $2",
                    params![club_id, tail],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                sqlx::query("UPDATE aircraft SET type_id = $1, active = $2 WHERE id = $3").bind(type_id).bind(i64::from(active)).bind(id).execute(&self.pool).await?;
                return Ok(id);
            }
            let id = new_id();
            sqlx::query("INSERT INTO aircraft (id, club_id, tail, type_id, active) VALUES ($1, $2, $3, $4, $5)").bind(id).bind(club_id).bind(tail).bind(type_id).bind(i64::from(active)).execute(&self.pool).await?;
            Ok(id)
        
    }

    pub async fn add_checkout(
        &self,
        pilot_id: &str,
        type_code: &str,
        tail: Option<&str>,
        expires_at: Option<i64>,
    ) -> Result<(), CheckoutError> {
        self.add_checkout_typed(pilot_id, type_code, tail, expires_at)
    }

    pub fn list_pending_pilots(&self) -> Vec<PilotRow> {
        
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
        
        .unwrap_or_default()
    }

    pub async fn list_types(&self, club_id: &str) -> Vec<TypeRow> {
        
            let mut stmt = conn
                .prepare("SELECT id, code FROM aircraft_types WHERE club_id = $1 ORDER BY code")?;
            let rows = stmt.query_map(params![club_id], |r| {
                Ok(TypeRow {
                    id: r.get(0)?,
                    code: r.get(1)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        
        .unwrap_or_default()
    }

    pub fn list_aircraft(&self, club_id: &str) -> Vec<AircraftRow> {
        
            let mut stmt = conn.prepare(
                "SELECT a.id, a.tail, t.code, a.active
                 FROM aircraft a JOIN aircraft_types t ON t.id = a.type_id
                 WHERE a.club_id = $1 ORDER BY a.tail",
            )?;
            let rows = stmt.query_map(params![club_id], |r| {
                Ok(AircraftRow {
                    id: r.get(0)?,
                    tail: r.get(1)?,
                    type_code: r.get(2)?,
                    active: r.get::<_, i64>(3)? != 0,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        
        .unwrap_or_default()
    }

    pub fn list_all_pilots(&self) -> Vec<PilotRow> {
        
            let mut stmt = conn.prepare(
                "SELECT id, club_id, name, email, license_no, status, role
                 FROM pilots ORDER BY email",
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
        
        .unwrap_or_default()
    }

    pub fn list_checkouts(&self) -> Vec<CheckoutRow> {
        
            let mut stmt = conn.prepare(
                "SELECT c.id, c.pilot_id, p.name, p.email, t.code, a.tail, c.expires_at
                 FROM checkouts c
                 JOIN pilots p ON p.id = c.pilot_id
                 JOIN aircraft_types t ON t.id = c.type_id
                 LEFT JOIN aircraft a ON a.id = c.aircraft_id
                 ORDER BY p.email, t.code",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(CheckoutRow {
                    id: r.get(0)?,
                    pilot_id: r.get(1)?,
                    pilot_name: r.get(2)?,
                    pilot_email: r.get(3)?,
                    type_code: r.get(4)?,
                    tail: r.get(5)?,
                    expires_at: r.get(6)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        
        .unwrap_or_default()
    }

    pub fn pilot_club_id(&self, pilot_id: &str) -> Result<String, DbError> {
        
            conn.query_row(
                "SELECT club_id FROM pilots WHERE id = $1",
                params![pilot_id],
                |r| r.get(0),
            )
        
    }

    pub fn club_checkin_settings(&self, club_id: &str) -> Result<(String, i64), DbError> {
        let (secret, minutes): (String, i64) = 
            conn.query_row(
                "SELECT checkin_hmac_secret, checkin_valid_minutes FROM clubs WHERE id = $1",
                params![club_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        ?;
        let secret = if secret.is_empty() {
            self.ensure_checkin_secret(club_id)?
        } else {
            secret
        };
        Ok((secret, minutes.max(1)))
    }

    pub async fn ensure_checkin_secret(&self, club_id: &str) -> Result<String, DbError> {
        if let Ok(env) = std::env::var("FLIGHT_CLUB_CHECKIN_SECRET") {
            if !env.is_empty() {
                
                    sqlx::query("UPDATE clubs SET checkin_hmac_secret = $1 WHERE id = $2").bind(env).bind(club_id).execute(&self.pool).await?                ?;
                return Ok(env);
            }
        }
        let secret = new_device_token();
        
            sqlx::query("UPDATE clubs SET checkin_hmac_secret = $1 WHERE id = $2").bind(secret).bind(club_id).execute(&self.pool).await?        ?;
        Ok(secret)
    }

    async fn insert_checkin_row(
        &self,
        club_id: &str,
        pilot_id: Option<&str>,
        device_id: Option<&str>,
        time_step: i64,
        token_digest: &str,
        now: i64,
        expires_at: Option<i64>,
        outcome: &str,
        fail_reason: Option<&str>,
    ) -> Result<String, DbError> {
        let id = new_id();
        
            sqlx::query("INSERT INTO checkins (id, club_id, pilot_id, device_id, time_step, token_digest, created_at, expires_at, outcome, fail_reason)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)").bind(id).bind(club_id).bind(pilot_id).bind(device_id).bind(time_step).bind(token_digest).bind(now).bind(expires_at).bind(outcome).bind(fail_reason).execute(&self.pool).await?;
            Ok(())
        ?;
        Ok(id)
    }

    pub async fn mint_checkin_qr_token(
        &self,
        club_id: &str,
        now: i64,
    ) -> Result<String, DbError> {
        let (secret, _) = self.club_checkin_settings(club_id)?;
        let step = checkin::current_step(now);
        Ok(checkin::mint_checkin_token(&secret, club_id, step))
    }

    fn checkin_replay(
        &self,
        pilot_id: &str,
        device_id: Option<&str>,
        token_digest: &str,
    ) -> Result<bool, DbError> {
        
            let count: i64 = if let Some(dev) = device_id {
                conn.query_row(
                    "SELECT COUNT(*) FROM checkins WHERE outcome = 'ok' AND pilot_id = $1 AND device_id = $2 AND token_digest = $3",
                    params![pilot_id, dev, token_digest],
                    |r| r.get(0),
                )?
            } else {
                conn.query_row(
                    "SELECT COUNT(*) FROM checkins WHERE outcome = 'ok' AND pilot_id = $1 AND device_id IS NULL AND token_digest = $2",
                    params![pilot_id, token_digest],
                    |r| r.get(0),
                )?
            };
            Ok(count > 0)
        
    }

    pub async fn submit_checkin(
        &self,
        pilot_id: &str,
        device_id: Option<&str>,
        token: &str,
        now: i64,
    ) -> Result<CheckinRecord, CheckinReject> {
        let pilot_club = self
            .pilot_club_id(pilot_id)
            .map_err(|_| CheckinReject::WrongClub)?;
        let (secret, valid_min) = self
            .club_checkin_settings(&pilot_club)
            .map_err(|_| CheckinReject::ForgedHmac)?;
        let digest = checkin::token_digest(token);
        let fail = |reason: &str, err: CheckinReject| {
            let _ = self.insert_checkin_row(
                &pilot_club,
                Some(pilot_id),
                device_id,
                0,
                &digest,
                now,
                None,
                "fail",
                Some(reason),
            );
            err
        };
        let (club_id, step) = match checkin::verify_checkin_token(&secret, token, now) {
            Ok(v) => v,
            Err(CheckinTokenError::Format) => return Err(fail("format", CheckinReject::Format)),
            Err(CheckinTokenError::Forged) => {
                return Err(fail("forged_hmac", CheckinReject::ForgedHmac))
            }
            Err(CheckinTokenError::OldStep) => {
                return Err(fail("old_step", CheckinReject::OldStep))
            }
        };
        if club_id != pilot_club {
            return Err(fail("wrong_club", CheckinReject::WrongClub));
        }
        if self
            .checkin_replay(pilot_id, device_id, &digest)
            .unwrap_or(false)
        {
            return Err(fail("replay", CheckinReject::Replay));
        }
        let expires = now + valid_min * 60_000;
        let id = self
            .insert_checkin_row(
                &pilot_club,
                Some(pilot_id),
                device_id,
                step,
                &digest,
                now,
                Some(expires),
                "ok",
                None,
            )
            .map_err(|_| CheckinReject::ForgedHmac)?;
        Ok(CheckinRecord {
            id,
            created_at: now,
            expires_at: expires,
        })
    }

    pub async fn consume_checkin(
        &self,
        pilot_id: &str,
        device_id: &str,
        club_id: &str,
        now: i64,
    ) -> Result<bool, DbError> {
        
            let checkin_id: Option<String> = conn
                .query_row(
                    "SELECT id FROM checkins
                     WHERE outcome = 'ok' AND pilot_id = $1 AND club_id = $2
                       AND consumed_at IS NULL AND expires_at IS NOT NULL AND expires_at > $3
                       AND (device_id IS NULL OR device_id = $4)
                     ORDER BY created_at DESC LIMIT 1",
                    params![pilot_id, club_id, now, device_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(id) = checkin_id else {
                return Ok(false);
            };
            sqlx::query("UPDATE checkins SET consumed_at = $1 WHERE id = $2 AND consumed_at IS NULL").bind(now).bind(id).execute(&self.pool).await?;
            Ok(true)
        
    }

    pub async fn issue_open_checkin(
        &self,
        pilot_id: &str,
        device_id: &str,
        club_id: &str,
        now: i64,
    ) -> Result<(), DbError> {
        let (_, valid_min) = self.club_checkin_settings(club_id)?;
        let expires = now + valid_min * 60_000;
        let digest = checkin::token_digest(&format!("internal:{pilot_id}:{now}"));
        let _ = self.insert_checkin_row(
            club_id,
            Some(pilot_id),
            Some(device_id),
            checkin::current_step(now),
            &digest,
            now,
            Some(expires),
            "ok",
            None,
        )?;
        Ok(())
    }

    pub async fn create_display_session(
        &self,
        admin_id: &str,
        label: &str,
    ) -> Result<(String, String), DbError> {
        let club_id = self.pilot_club_id(admin_id)?;
        let token = new_device_token();
        let hash = digest_token(&token);
        let id = new_id();
        let now = now_ms();
        
            sqlx::query("INSERT INTO display_sessions (id, club_id, created_by_pilot_id, token_hash, label, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6)").bind(id).bind(club_id).bind(admin_id).bind(hash).bind(label).bind(now).execute(&self.pool).await?        ?;
        Ok((id, token))
    }

    pub async fn revoke_display_session(
        &self,
        admin_id: &str,
        session_id: &str,
    ) -> Result<bool, DbError> {
        let club_id = self.pilot_club_id(admin_id)?;
        let updated = 
            sqlx::query("UPDATE display_sessions SET revoked_at = $1
                 WHERE id = $2 AND club_id = $3 AND created_by_pilot_id = $4 AND revoked_at IS NULL").bind(now_ms()).bind(session_id).bind(club_id).bind(admin_id).execute(&self.pool).await?        ?;
        Ok(updated > 0)
    }

    pub fn resolve_display_session(&self, token: &str) -> Option<(String, String, String)> {
        let hash = digest_token(token);
        let row = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT id, club_id, created_by_pilot_id FROM display_sessions
                     WHERE token_hash = $1 AND revoked_at IS NULL",
                    params![hash],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
            })
            .ok()
            .flatten()?;
        Some(row)
    }

    pub fn list_display_sessions(&self, admin_id: &str) -> Vec<DisplaySessionRow> {
        
            let mut stmt = conn.prepare(
                "SELECT id, label, created_at, revoked_at FROM display_sessions
                 WHERE created_by_pilot_id = $1 ORDER BY created_at DESC",
            )?;
            let rows = stmt.query_map(params![admin_id], |r| {
                let revoked_at: Option<i64> = r.get(3)?;
                Ok(DisplaySessionRow {
                    id: r.get(0)?,
                    label: r.get(1)?,
                    created_at: r.get(2)?,
                    revoked: revoked_at.is_some(),
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        
        .unwrap_or_default()
    }

    pub async fn display_map(&self, admin_creator_id: &str) -> Vec<TailLivePosition> {
        let rows: Vec<DeviceFix> = self
            .latest_device_fixes()
            .into_iter()
            .filter(|r| {
                let vis = FlightVisibility::parse(&r.visibility).unwrap_or(FlightVisibility::Club);
                matches!(vis, FlightVisibility::Club | FlightVisibility::Public)
            })
            .collect();
        let mut positions = dedupe_by_tail(rows);
        for pos in &mut positions {
            if let Some(f) = self.get_flight(&pos.flight_id) {
                if show_pilot_name(&Viewer::Anonymous, &f.pilot_id, f.show_pilot_name_public) {
                    pos.pilot_name = Some(f.pilot_name);
                } else {
                    pos.pilot_name = None;
                }
            }
        }
        let prefs = self.admin_map_tails(admin_creator_id);
        filter_admin_tails(positions, &prefs)
    }

    pub fn seed_demo_if_empty(&self) -> Result<(), DbError> {
        let club_id = self.default_club_id()?;
        let exists: i64 = 
            conn.query_row(
                "SELECT COUNT(*) FROM pilots WHERE email = 'admin@demo.test'",
                [],
                |r| r.get(0),
            )
        ?;
        if exists > 0 {
            return Ok(());
        }
        let admin_id = new_id();
        let pilot_id = new_id();
        let pending_id = new_id();
        let hash = auth::hash_password("password12")
            .map_err(|_| DbError::InvalidParameterName("hash".into()))?;
        let t = now_ms();
        
            sqlx::query("INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at) VALUES
                 ($1, $4, 'Demo Admin', 'admin@demo.test', 'ADM-1', 'approved', 'admin', $5, $6),
                 ($2, $4, 'Demo Pilot', 'pilot@demo.test', 'P-1', 'approved', 'pilot', $5, $6),
                 ($3, $4, 'Demo Pending', 'pending@demo.test', 'P-2', 'pending', 'pilot', $5, $6)").bind(admin_id).bind(pilot_id).bind(pending_id).bind(club_id).bind(hash).bind(t).execute(&self.pool).await?;
            Ok(())
        ?;
        for code in ["DA20-C1", "C152"] {
            self.upsert_type(&club_id, code)?;
        }
        for tail in ["N101", "N202", "N303"] {
            self.upsert_aircraft(&club_id, tail, "C152", true)?;
        }
        self.add_checkout(&pilot_id, "C152", None, None)?;
        self.add_checkout(&admin_id, "C152", None, None)?;
        let (dev_pilot, _) = self.create_device(&pilot_id, "Pilot phone")?;
        let (dev_admin, _) = self.create_device(&admin_id, "Admin tablet")?;
        self.issue_open_checkin(&pilot_id, &dev_pilot, &club_id, t)?;
        self.issue_open_checkin(&admin_id, &dev_admin, &club_id, t)?;
        let seed_flight_err = |e: StartFlightError| {
            DbError::InvalidParameterName(format!("seed_demo flight: {e:?}"))
        };
        let pub_id = self
            .start_flight(
                &pilot_id,
                &dev_pilot,
                "N101",
                FlightVisibility::Public,
                true,
                t,
            )
            .map_err(seed_flight_err)?;
        let club_f = self
            .start_flight(
                &admin_id,
                &dev_admin,
                "N202",
                FlightVisibility::Club,
                false,
                t,
            )
            .map_err(seed_flight_err)?;
        let fix = |offset: i64, lat: f64| LiveFix {
            t: t + offset,
            lat,
            lon: -70.6,
            alt: Some(900.0),
            alt_ref: None,
            speed: Some(28.0),
            course: Some(120.0),
            accuracy: Some(6.0),
        };
        self.append_fix(&pub_id, &dev_pilot, &fix(100, -33.40))?;
        self.append_fix(&pub_id, &dev_pilot, &fix(200, -33.39))?;
        self.append_fix(&club_f, &dev_admin, &fix(150, -33.38))?;
        self.append_fix(&club_f, &dev_admin, &fix(250, -33.37))?;
        Ok(())
    }

    // from db_checkin.rs
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
        if !(1..=24 * 365).contains(&hours) {
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
        if !(1..=24 * 365).contains(&hours) {
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

    // from db_devices.rs
#[allow(dead_code)]
    pub fn count_devices_for_pilot(&self, pilot_id: &str) -> usize {
        self.with_conn(|conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM devices WHERE pilot_id = ?1",
                params![pilot_id],
                |r| r.get(0),
            )?;
            Ok(n as usize)
        })
        .unwrap_or(0)
    }

    /// App auto-registration: stable device row per `(pilot_id, install_id)`, fresh bearer token each call.
    pub fn register_device_for_install(
        &self,
        pilot_id: &str,
        label: &str,
        model: Option<&str>,
        install_id: &str,
    ) -> Result<DeviceRegisterResponse, DeviceRegisterError> {
        let install_id = install_id.trim();
        if install_id.len() < MIN_INSTALL_ID_LEN || install_id.len() > MAX_INSTALL_ID_LEN {
            return Err(DeviceRegisterError::InvalidInstallId);
        }
        let label = if label.trim().is_empty() {
            "Dispositivo"
        } else {
            label.trim()
        };
        let model = model.map(str::trim).filter(|s| !s.is_empty());
        let token = new_device_token();
        let hash = digest_token(&token);
        let now = now_ms();

        self.with_conn_any(|conn| -> Result<DeviceRegisterResponse, DeviceRegisterError> {
            if let Some(existing_id) = conn
                .query_row(
                    "SELECT id FROM devices WHERE pilot_id = ?1 AND install_id = ?2",
                    params![pilot_id, install_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(|_| DeviceRegisterError::Db)?
            {
                conn.execute(
                    "UPDATE devices SET token_hash = ?1, label = ?2, model = ?3 WHERE id = ?4",
                    params![hash, label, model, existing_id],
                )
                .map_err(|_| DeviceRegisterError::Db)?;
                conn.execute(
                    "DELETE FROM device_pair_tokens WHERE device_id = ?1",
                    params![existing_id],
                )
                .ok();
                return Ok(DeviceRegisterResponse {
                    id: existing_id,
                    token,
                    label: label.to_string(),
                    model: model.map(String::from),
                    install_id: Some(install_id.to_string()),
                    created: false,
                });
            }
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM devices WHERE pilot_id = ?1",
                    params![pilot_id],
                    |r| r.get(0),
                )
                .map_err(|_| DeviceRegisterError::Db)?;
            if count as usize >= MAX_DEVICES_PER_PILOT {
                return Err(DeviceRegisterError::CapExceeded);
            }
            let id = new_id();
            conn.execute(
                "INSERT INTO devices (id, pilot_id, token_hash, label, model, install_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, pilot_id, hash, label, model, install_id, now],
            )
            .map_err(|_| DeviceRegisterError::Db)?;
            Ok(DeviceRegisterResponse {
                id,
                token,
                label: label.to_string(),
                model: model.map(String::from),
                install_id: Some(install_id.to_string()),
                created: true,
            })
        })
    }

    fn end_active_flights_for_device(
        conn: &rusqlite::Connection,
        pilot_id: &str,
        device_id: &str,
        now: i64,
    ) -> rusqlite::Result<()> {
        let mut stmt = conn.prepare(
            "SELECT f.id FROM flights f
             INNER JOIN flight_devices fd ON fd.flight_id = f.id
             WHERE fd.device_id = ?1 AND f.pilot_id = ?2 AND f.status = 'active'",
        )?;
        let ids: Vec<String> = stmt
            .query_map(params![device_id, pilot_id], |r| r.get(0))?
            .filter_map(Result::ok)
            .collect();
        for flight_id in ids {
            conn.execute(
                "UPDATE flights SET status = 'ended', ended_at = ?1 WHERE id = ?2",
                params![now, flight_id],
            )?;
        }
        Ok(())
    }

    fn revoke_device_row(
        conn: &rusqlite::Connection,
        pilot_id: &str,
        device_id: &str,
    ) -> rusqlite::Result<bool> {
        let history: i64 = conn.query_row(
            "SELECT COUNT(*) FROM flight_devices WHERE device_id = ?1",
            params![device_id],
            |r| r.get(0),
        )?;
        if history == 0 {
            let deleted = conn.execute(
                "DELETE FROM devices WHERE pilot_id = ?1 AND id = ?2",
                params![pilot_id, device_id],
            )?;
            return Ok(deleted > 0);
        }
        let revoked_hash = digest_token(&format!("revoked-{}", new_id()));
        let updated = conn.execute(
            "UPDATE devices SET token_hash = ?1 WHERE pilot_id = ?2 AND id = ?3",
            params![revoked_hash, pilot_id, device_id],
        )?;
        conn.execute(
            "DELETE FROM device_pair_tokens WHERE device_id = ?1",
            params![device_id],
        )
        .ok();
        Ok(updated > 0)
    }

    pub fn revoke_device_by_install(
        &self,
        pilot_id: &str,
        install_id: &str,
    ) -> Result<bool, rusqlite::Error> {
        let install_id = install_id.trim();
        self.with_conn(|conn| {
            let device_id: Option<String> = conn
                .query_row(
                    "SELECT id FROM devices WHERE pilot_id = ?1 AND install_id = ?2",
                    params![pilot_id, install_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(device_id) = device_id else {
                return Ok(false);
            };
            Self::end_active_flights_for_device(conn, pilot_id, &device_id, now_ms())?;
            Self::revoke_device_row(conn, pilot_id, &device_id)
        })
    }

    pub fn revoke_device_by_id(
        &self,
        pilot_id: &str,
        device_id: &str,
    ) -> Result<bool, rusqlite::Error> {
        self.with_conn(|conn| {
            Self::end_active_flights_for_device(conn, pilot_id, device_id, now_ms())?;
            Self::revoke_device_row(conn, pilot_id, device_id)
        })
    }

    // from db_map_view.rs
pub fn dem_index(&self) -> DemIndex {
        DemIndex::new(self.data_dir())
    }

    pub fn pilot_is_approved_member(&self, pilot_id: &str, club_id: &str) -> bool {
        self.with_conn(|conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM club_memberships
                 WHERE pilot_id = ?1 AND club_id = ?2 AND status = 'approved'",
                params![pilot_id, club_id],
                |r| r.get(0),
            )?;
            Ok(n > 0)
        })
        .unwrap_or(false)
    }

    pub fn club_map_filter(&self, club_id: &str) -> MapViewFilter {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT COALESCE(latitude, 0), COALESCE(longitude, 0), public_radius_km,
                        show_public_tracks, public_flights_cap, public_altitude_mode, public_altitude_min_ft
                 FROM clubs WHERE id = ?1",
                params![club_id],
                |r| {
                    Ok(MapViewFilter {
                        center_lat: r.get(0)?,
                        center_lon: r.get(1)?,
                        radius_km: r.get(2)?,
                        show_public_tracks: r.get::<_, i64>(3)? != 0,
                        cap: r.get::<_, i64>(4)? as usize,
                        altitude_mode: AltitudeMode::parse(&r.get::<_, String>(5)?),
                        altitude_min_ft: r.get(6)?,
                        club_id: club_id.to_string(),
                    })
                },
            )
        })
        .unwrap_or_else(|_| MapViewFilter {
            center_lat: 0.0,
            center_lon: 0.0,
            radius_km: 100.0,
            show_public_tracks: true,
            cap: 50,
            altitude_mode: AltitudeMode::Agl,
            altitude_min_ft: 300.0,
            club_id: club_id.to_string(),
        })
    }

    pub fn user_map_filter(&self, pilot_id: &str, club_id: &str) -> MapViewFilter {
        let mut base = self.club_map_filter(club_id);
        // User prefs can tighten public radius/cap/altitude; inherit club show_public_tracks unless overridden per-user column.
        if let Ok((radius, show, cap, mode, min_ft, lat, lon, club_lat, club_lon)) =
            self.with_conn(|conn| {
                conn.query_row(
                    "SELECT p.public_radius_km, p.show_public_tracks, p.public_flights_cap,
                        p.public_altitude_mode, p.public_altitude_min_ft,
                        COALESCE(p.last_latitude, c.latitude, 0),
                        COALESCE(p.last_longitude, c.longitude, 0),
                        COALESCE(c.latitude, 0), COALESCE(c.longitude, 0)
                 FROM pilots p
                 JOIN clubs c ON c.id = ?2
                 WHERE p.id = ?1",
                    params![pilot_id, club_id],
                    |r| {
                        Ok((
                            r.get::<_, f64>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, f64>(4)?,
                            r.get::<_, f64>(5)?,
                            r.get::<_, f64>(6)?,
                            r.get::<_, f64>(7)?,
                            r.get::<_, f64>(8)?,
                        ))
                    },
                )
            })
        {
            base.radius_km = radius;
            base.show_public_tracks = show != 0;
            base.cap = cap as usize;
            base.altitude_mode = AltitudeMode::parse(&mode);
            base.altitude_min_ft = min_ft;
            if lat != 0.0 || lon != 0.0 {
                base.center_lat = lat;
                base.center_lon = lon;
            } else if club_lat != 0.0 || club_lon != 0.0 {
                base.center_lat = club_lat;
                base.center_lon = club_lon;
            }
        }
        base
    }

    fn latest_device_fixes_bbox(
        &self,
        min_lat: f64,
        max_lat: f64,
        min_lon: f64,
        max_lon: f64,
    ) -> Vec<DeviceFix> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT f.id, a.tail, f.pilot_id, p.name, f.visibility, a.club_id,
                        fx.t_ms, fx.lat, fx.lon, fx.alt, fx.alt_ref, fx.speed, fx.course, fx.accuracy
                 FROM flights f
                 JOIN aircraft a ON a.id = f.aircraft_id
                 JOIN pilots p ON p.id = f.pilot_id
                 JOIN fixes fx ON fx.flight_id = f.id
                 WHERE f.status = 'active'
                   AND fx.lat BETWEEN ?1 AND ?2
                   AND fx.lon BETWEEN ?3 AND ?4
                   AND fx.t_ms = (
                     SELECT MAX(fx2.t_ms) FROM fixes fx2 WHERE fx2.flight_id = f.id
                   )",
            )?;
            let rows = stmt.query_map(params![min_lat, max_lat, min_lon, max_lon], |r| {
                Ok(DeviceFix {
                    flight_id: r.get(0)?,
                    tail: r.get(1)?,
                    pilot_id: r.get(2)?,
                    pilot_name: r.get(3)?,
                    visibility: r.get(4)?,
                    club_id: r.get(5)?,
                    fix: crate::live::LiveFix {
                        t: r.get(6)?,
                        lat: r.get(7)?,
                        lon: r.get(8)?,
                        alt: r.get(9)?,
                        alt_ref: r.get(10)?,
                        speed: r.get(11)?,
                        course: r.get(12)?,
                        accuracy: r.get(13)?,
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

    fn latest_device_fixes_unbounded(&self) -> Vec<DeviceFix> {
        self.latest_device_fixes_bbox(-90.0, 90.0, -180.0, 180.0)
    }

    pub fn latest_map_fixes_global(&self) -> Vec<DeviceFix> {
        self.latest_device_fixes_unbounded()
    }

    fn apply_public_view_policy(
        &self,
        rows: Vec<DeviceFix>,
        viewer: &Viewer,
        viewing_club_id: &str,
        filter: &MapViewFilter,
        dem: &DemIndex,
    ) -> Vec<DeviceFix> {
        rows.into_iter()
            .filter(|r| {
                let vis = FlightVisibility::parse(&r.visibility).unwrap_or(FlightVisibility::Club);
                if self.pilot_is_approved_member(&r.pilot_id, viewing_club_id) {
                    return match viewer {
                        Viewer::Pilot {
                            status: PilotStatus::Approved,
                            ..
                        } => {
                            includes_in_club_map(viewer, &r.pilot_id, vis)
                                && can_view_flight(viewer, &r.pilot_id, vis)
                        }
                        _ => false,
                    };
                }
                if !filter.show_public_tracks || !visibility_is_public(&r.visibility) {
                    return false;
                }
                public_track_passes(r, filter, dem)
            })
            .collect()
    }

    pub fn club_map_for_viewer(&self, viewer: &Viewer) -> Vec<TailLivePosition> {
        let (viewing_club, filter) = match viewer {
            Viewer::Pilot {
                id,
                status: PilotStatus::Approved,
                ..
            } => {
                let club = self.pilot_club_id(id).unwrap_or_default();
                (club.clone(), self.user_map_filter(id, &club))
            }
            Viewer::Anonymous
            | Viewer::Pilot {
                status: PilotStatus::Pending,
                ..
            } => {
                let club = self.default_club_id().unwrap_or_default();
                (club.clone(), self.club_map_filter(&club))
            }
        };
        let dem = self.dem_index();
        let rows = self.latest_map_fixes_global();
        let filtered = self.apply_public_view_policy(rows, viewer, &viewing_club, &filter, &dem);
        let mut positions = dedupe_by_tail(filtered);
        for pos in &mut positions {
            if let Some(f) = self.get_flight(&pos.flight_id) {
                pos.pilot_name = Some(f.pilot_name);
            }
        }
        apply_public_cap(
            positions,
            filter.cap,
            (filter.center_lat, filter.center_lon),
        )
    }

    pub fn public_map(&self) -> Vec<TailLivePosition> {
        let club = self.default_club_id().unwrap_or_default();
        let filter = self.club_map_filter(&club);
        let dem = self.dem_index();
        let rows = self.latest_map_fixes_global();
        let filtered: Vec<DeviceFix> = rows
            .into_iter()
            .filter(|r| {
                if self.pilot_is_approved_member(&r.pilot_id, &club) {
                    return includes_in_public_map(
                        FlightVisibility::parse(&r.visibility).unwrap_or(FlightVisibility::Club),
                    );
                }
                filter.show_public_tracks
                    && visibility_is_public(&r.visibility)
                    && public_track_passes(r, &filter, &dem)
            })
            .collect();
        let mut positions = dedupe_by_tail(filtered);
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
        apply_public_cap(
            positions,
            filter.cap,
            (filter.center_lat, filter.center_lon),
        )
    }

    pub fn flight_visible_to(&self, viewer: &Viewer, flight_id: &str) -> Option<FlightRow> {
        let flight = self.get_flight(flight_id)?;
        let vis = FlightVisibility::parse(&flight.visibility)?;
        if !can_view_flight(viewer, &flight.pilot_id, vis) {
            return None;
        }
        let club_id = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT a.club_id FROM flights f JOIN aircraft a ON a.id = f.aircraft_id WHERE f.id = ?1",
                    params![flight_id],
                    |r| r.get::<_, String>(0),
                )
            })
            .ok()?;
        let viewing_club = match viewer {
            Viewer::Pilot {
                id,
                status: PilotStatus::Approved,
                ..
            } => self.pilot_club_id(id).unwrap_or_else(|_| club_id.clone()),
            Viewer::Anonymous | Viewer::Pilot { .. } => {
                self.default_club_id().unwrap_or_else(|_| club_id.clone())
            }
        };
        if self.pilot_is_approved_member(&flight.pilot_id, &viewing_club) {
            return Some(flight);
        }
        if vis != FlightVisibility::Public {
            return None;
        }
        let filter = match viewer {
            Viewer::Pilot {
                id,
                status: PilotStatus::Approved,
                ..
            } => self.user_map_filter(id, &club_id),
            Viewer::Pilot { .. } | Viewer::Anonymous => self.club_map_filter(&club_id),
        };
        if !filter.show_public_tracks {
            return None;
        }
        let dem = self.dem_index();
        let latest: Option<DeviceFix> = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT a.tail, f.pilot_id, p.name, f.visibility, a.club_id,
                            fx.t_ms, fx.lat, fx.lon, fx.alt, fx.alt_ref, fx.speed, fx.course, fx.accuracy
                     FROM flights f
                     JOIN aircraft a ON a.id = f.aircraft_id
                     JOIN pilots p ON p.id = f.pilot_id
                     JOIN fixes fx ON fx.flight_id = f.id
                     WHERE f.id = ?1
                     ORDER BY fx.t_ms DESC LIMIT 1",
                    params![flight_id],
                    |r| {
                        Ok(DeviceFix {
                            flight_id: flight_id.to_string(),
                            tail: r.get(0)?,
                            pilot_id: r.get(1)?,
                            pilot_name: r.get(2)?,
                            visibility: r.get(3)?,
                            club_id: r.get(4)?,
                            fix: crate::live::LiveFix {
                                t: r.get(5)?,
                                lat: r.get(6)?,
                                lon: r.get(7)?,
                                alt: r.get(8)?,
                                alt_ref: r.get(9)?,
                                speed: r.get(10)?,
                                course: r.get(11)?,
                                accuracy: r.get(12)?,
                            },
                        })
                    },
                )
                .optional()
            })
            .ok()
            .flatten();
        let row = latest?;
        if public_track_passes(&row, &filter, &dem) {
            Some(flight)
        } else {
            None
        }
    }

    // from db_kiosk_layout.rs
pub fn kiosk_qr_layout(&self, club_id: &str) -> Result<KioskQrLayoutRow, rusqlite::Error> {
        self.with_conn(|conn| {
            let row = conn.query_row(
                    "SELECT kiosk_qr_x, kiosk_qr_y, kiosk_qr_size, kiosk_layout_rev FROM clubs WHERE id = ?1",
                    params![club_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                );
            let (x, y, size, rev): (Option<f64>, Option<f64>, Option<f64>, i64) = row?;
            let layout = match (x, y, size) {
                (Some(x), Some(y), Some(size)) => {
                    clamp_kiosk_qr_layout(x, y, size, DEFAULT_PREVIEW_ASPECT)
                }
                _ => default_kiosk_qr_layout(DEFAULT_PREVIEW_ASPECT),
            };
            Ok(KioskQrLayoutRow {
                x: layout.x,
                y: layout.y,
                size: layout.size,
                rev,
            })
        })
    }

    pub fn set_kiosk_qr_layout(
        &self,
        club_id: &str,
        x: f64,
        y: f64,
        size: f64,
        aspect: f64,
    ) -> Result<KioskQrLayoutRow, rusqlite::Error> {
        let layout = clamp_kiosk_qr_layout(x, y, size, aspect);
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE clubs SET kiosk_qr_x = ?1, kiosk_qr_y = ?2, kiosk_qr_size = ?3,
                 kiosk_layout_rev = kiosk_layout_rev + 1 WHERE id = ?4",
                params![layout.x, layout.y, layout.size, club_id],
            )?;
            Ok(())
        })?;
        self.kiosk_qr_layout(club_id)
    }

    pub fn reset_kiosk_qr_layout(
        &self,
        club_id: &str,
        aspect: f64,
    ) -> Result<KioskQrLayoutRow, rusqlite::Error> {
        let layout = default_kiosk_qr_layout(aspect);
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE clubs SET kiosk_qr_x = ?1, kiosk_qr_y = ?2, kiosk_qr_size = ?3,
                 kiosk_layout_rev = kiosk_layout_rev + 1 WHERE id = ?4",
                params![layout.x, layout.y, layout.size, club_id],
            )?;
            Ok(())
        })?;
        self.kiosk_qr_layout(club_id)
    }

    pub fn ensure_kiosk_qr_defaults(&self, club_id: &str) -> Result<(), rusqlite::Error> {
        self.with_conn(|conn| {
            let needs: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM clubs WHERE id = ?1 AND kiosk_qr_x IS NULL",
                    params![club_id],
                    |r| r.get(0),
                )
                .optional()?;
            if needs.is_some() {
                let d = default_kiosk_qr_layout(DEFAULT_PREVIEW_ASPECT);
                conn.execute(
                    "UPDATE clubs SET kiosk_qr_x = ?1, kiosk_qr_y = ?2, kiosk_qr_size = ?3 WHERE id = ?4",
                    params![d.x, d.y, d.size, club_id],
                )?;
            }
            Ok(())
        })
    }

    // from db_shares.rs
pub fn create_flight_share(
        &self,
        pilot_id: &str,
        flight_id: &str,
    ) -> Result<(FlightShareRow, String), ShareCreateError> {
        let flight = self
            .get_flight(flight_id)
            .ok_or(ShareCreateError::NotFound)?;
        if flight.pilot_id != pilot_id {
            return Err(ShareCreateError::NotOwner);
        }
        let club_id = self.with_conn(|conn| {
            conn.query_row(
                "SELECT a.club_id FROM flights f JOIN aircraft a ON a.id = f.aircraft_id WHERE f.id = ?1",
                params![flight_id],
                |r| r.get::<_, String>(0),
            )
        })
        .map_err(ShareCreateError::Db)?;
        let token = new_share_token();
        let token_hash = digest_token(&token);
        let id = new_id();
        let created_at = now_ms();
        let row = FlightShareRow {
            id: id.clone(),
            flight_id: flight_id.to_string(),
            club_id,
            created_by: pilot_id.to_string(),
            token_hash: token_hash.clone(),
            created_at,
            expires_at: None,
            revoked_at: None,
        };
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO flight_shares (id, flight_id, club_id, created_by, token_hash, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    row.id,
                    row.flight_id,
                    row.club_id,
                    row.created_by,
                    row.token_hash,
                    row.created_at,
                ],
            )
        })
        .map_err(ShareCreateError::Db)?;
        Ok((row, token))
    }

    pub fn list_flight_shares(
        &self,
        pilot_id: &str,
        flight_id: &str,
    ) -> Result<Vec<FlightShareListItem>, ShareCreateError> {
        let flight = self
            .get_flight(flight_id)
            .ok_or(ShareCreateError::NotFound)?;
        if flight.pilot_id != pilot_id {
            return Err(ShareCreateError::NotOwner);
        }
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, created_at, expires_at, revoked_at FROM flight_shares
                 WHERE flight_id = ?1 ORDER BY created_at DESC",
            )?;
            let rows = stmt.query_map(params![flight_id], |r| {
                Ok(FlightShareListItem {
                    id: r.get(0)?,
                    created_at: r.get(1)?,
                    expires_at: r.get(2)?,
                    revoked: r.get::<_, Option<i64>>(3)?.is_some(),
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(ShareCreateError::Db)
    }

    pub fn revoke_flight_share(
        &self,
        pilot_id: &str,
        flight_id: &str,
        share_id: &str,
    ) -> Result<(), ShareRevokeError> {
        let flight = self
            .get_flight(flight_id)
            .ok_or(ShareRevokeError::NotFound)?;
        if flight.pilot_id != pilot_id {
            return Err(ShareRevokeError::NotOwner);
        }
        let now = now_ms();
        let changed = self
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE flight_shares SET revoked_at = ?1
                     WHERE id = ?2 AND flight_id = ?3 AND revoked_at IS NULL",
                    params![now, share_id, flight_id],
                )
            })
            .map_err(ShareRevokeError::Db)?;
        if changed == 0 {
            return Err(ShareRevokeError::NotFound);
        }
        Ok(())
    }

    fn fetch_share_by_hash(&self, token_hash: &str) -> Option<FlightShareRow> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT id, flight_id, club_id, created_by, token_hash, created_at, expires_at, revoked_at
                 FROM flight_shares WHERE token_hash = ?1",
                params![token_hash],
                |r| {
                    Ok(row_from_query(
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )
            .optional()
        })
        .ok()
        .flatten()
    }

    pub fn resolve_share_token(&self, token: &str) -> Option<ResolvedShare> {
        if token.is_empty() || token.len() > 128 {
            return None;
        }
        let digest = digest_token(token);
        let share = self.fetch_share_by_hash(&digest)?;
        if !token_hash_matches(&share.token_hash, &digest) {
            return None;
        }
        let flight = self.get_flight(&share.flight_id)?;
        if !share_active_at(&share, &flight, now_ms()) {
            return None;
        }
        Some(ResolvedShare {
            flight_id: flight.id,
            club_id: share.club_id,
            tail: flight.tail,
            status: flight.status.clone(),
        })
    }

    /// Same flight as resolve_share_token; used to assert share access does not leak other flights.
    #[allow(dead_code)]
    pub fn share_flight_id(&self, token: &str) -> Option<String> {
        self.resolve_share_token(token).map(|r| r.flight_id)
    }

    // from db_access.rs
pub fn record_audit(
        &self,
        club_id: &str,
        actor_pilot_id: Option<&str>,
        action: &str,
        entity_type: &str,
        entity_id: &str,
        detail: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        let now = now_ms();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    new_id(),
                    club_id,
                    actor_pilot_id,
                    action,
                    entity_type,
                    entity_id,
                    detail,
                    now
                ],
            )?;
            Ok(())
        })
    }

    pub fn pilot_has_open_checkin(&self, pilot_id: &str, now: i64) -> bool {
        self.with_conn(|conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM checkins
                 WHERE outcome = 'ok' AND pilot_id = ?1
                   AND consumed_at IS NULL AND expires_at IS NOT NULL AND expires_at > ?2",
                params![pilot_id, now],
                |r| r.get(0),
            )?;
            Ok(n > 0)
        })
        .unwrap_or(false)
    }

    pub fn count_pending_access_requests(&self, club_id: &str) -> usize {
        self.with_conn(|conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM access_requests WHERE club_id = ?1 AND status = 'pending'",
                params![club_id],
                |r| r.get(0),
            )?;
            Ok(n as usize)
        })
        .unwrap_or(0)
    }

    pub fn create_access_request(
        &self,
        pilot_id: &str,
        tail: &str,
        type_id: Option<&str>,
        new_type_name: Option<&str>,
        now: i64,
    ) -> Result<String, AccessRequestError> {
        let tail = normalize_tail(tail).map_err(|_| AccessRequestError::InvalidTail)?;
        if type_id.is_none() && new_type_name.map(str::trim).unwrap_or("").is_empty() {
            return Err(AccessRequestError::MissingType);
        }
        if !self.pilot_has_open_checkin(pilot_id, now) {
            return Err(AccessRequestError::NoCheckin);
        }
        let club_id: String = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT club_id FROM pilots WHERE id = ?1",
                    params![pilot_id],
                    |r| r.get(0),
                )
            })
            .map_err(|_| AccessRequestError::Db)?;
        if let Some(tid) = type_id {
            let ok = self
                .with_conn(|conn| {
                    let n: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM aircraft_types WHERE club_id = ?1 AND id = ?2",
                        params![club_id, tid],
                        |r| r.get(0),
                    )?;
                    Ok(n > 0)
                })
                .unwrap_or(false);
            if !ok {
                return Err(AccessRequestError::MissingType);
            }
        }
        let dup: i64 = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM access_requests
                     WHERE pilot_id = ?1 AND requested_tail = ?2 AND status = 'pending'",
                    params![pilot_id, tail],
                    |r| r.get(0),
                )
            })
            .unwrap_or(0);
        if dup > 0 {
            return Err(AccessRequestError::DuplicatePending);
        }
        let id = new_id();
        let new_type = new_type_name.map(str::trim).filter(|s| !s.is_empty());
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO access_requests (id, club_id, pilot_id, requested_tail, type_id, new_type_name, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
                params![id, club_id, pilot_id, tail, type_id, new_type, now],
            )?;
            Ok(())
        })
        .map_err(|_| AccessRequestError::Db)?;
        let _ = self.record_audit(
            &club_id,
            Some(pilot_id),
            "access_request.create",
            "access_request",
            &id,
            Some(&tail),
        );
        Ok(id)
    }

    pub fn list_pilot_access_requests(&self, pilot_id: &str) -> Vec<AccessRequestView> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT ar.id, ar.pilot_id, ar.requested_tail, t.code, ar.new_type_name,
                        ar.status, ar.reject_reason, ar.created_at, ar.decided_at
                 FROM access_requests ar
                 LEFT JOIN aircraft_types t ON t.id = ar.type_id
                 WHERE ar.pilot_id = ?1
                 ORDER BY ar.created_at DESC",
            )?;
            let rows = stmt.query_map(params![pilot_id], |r| {
                Ok(AccessRequestView {
                    id: r.get(0)?,
                    requested_tail: r.get(2)?,
                    type_code: r.get(3)?,
                    new_type_name: r.get(4)?,
                    status: r.get(5)?,
                    reject_reason: r.get(6)?,
                    created_at: r.get(7)?,
                    decided_at: r.get(8)?,
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

    pub fn list_admin_access_requests(
        &self,
        club_id: &str,
        status: Option<&str>,
    ) -> Vec<AdminAccessRequestView> {
        self.with_conn(|conn| {
            let sql = if status.is_some() {
                "SELECT ar.id, ar.pilot_id, ar.requested_tail, t.code, ar.new_type_name,
                        ar.status, ar.reject_reason, ar.created_at, ar.decided_at,
                        ar.pilot_id, p.name, p.email
                 FROM access_requests ar
                 JOIN pilots p ON p.id = ar.pilot_id
                 LEFT JOIN aircraft_types t ON t.id = ar.type_id
                 WHERE ar.club_id = ?1 AND ar.status = ?2
                 ORDER BY ar.created_at DESC"
            } else {
                "SELECT ar.id, ar.pilot_id, ar.requested_tail, t.code, ar.new_type_name,
                        ar.status, ar.reject_reason, ar.created_at, ar.decided_at,
                        ar.pilot_id, p.name, p.email
                 FROM access_requests ar
                 JOIN pilots p ON p.id = ar.pilot_id
                 LEFT JOIN aircraft_types t ON t.id = ar.type_id
                 WHERE ar.club_id = ?1
                 ORDER BY ar.created_at DESC"
            };
            let mut stmt = conn.prepare(sql)?;
            let mut out = Vec::new();
            if let Some(st) = status {
                let rows = stmt.query_map(params![club_id, st], |r| {
                    Ok(AdminAccessRequestView {
                        base: AccessRequestView {
                            id: r.get(0)?,
                            requested_tail: r.get(2)?,
                            type_code: r.get(3)?,
                            new_type_name: r.get(4)?,
                            status: r.get(5)?,
                            reject_reason: r.get(6)?,
                            created_at: r.get(7)?,
                            decided_at: r.get(8)?,
                        },
                        pilot_id: r.get(9)?,
                        pilot_name: r.get(10)?,
                        pilot_email: r.get(11)?,
                    })
                })?;
                for row in rows {
                    out.push(row?);
                }
            } else {
                let rows = stmt.query_map(params![club_id], |r| {
                    Ok(AdminAccessRequestView {
                        base: AccessRequestView {
                            id: r.get(0)?,
                            requested_tail: r.get(2)?,
                            type_code: r.get(3)?,
                            new_type_name: r.get(4)?,
                            status: r.get(5)?,
                            reject_reason: r.get(6)?,
                            created_at: r.get(7)?,
                            decided_at: r.get(8)?,
                        },
                        pilot_id: r.get(9)?,
                        pilot_name: r.get(10)?,
                        pilot_email: r.get(11)?,
                    })
                })?;
                for row in rows {
                    out.push(row?);
                }
            }
            Ok(out)
        })
        .unwrap_or_default()
    }

    pub fn approve_access_request(
        &self,
        admin_id: &str,
        request_id: &str,
        expires_at: Option<i64>,
        now: i64,
    ) -> Result<(), AccessRequestError> {
        self.with_conn_any(|conn| -> Result<(), AccessRequestError> {
            let tx = conn.unchecked_transaction().map_err(|_| AccessRequestError::Db)?;
            let row = conn
                .query_row(
                    "SELECT ar.club_id, ar.pilot_id, ar.requested_tail, ar.type_id, ar.new_type_name, ar.status
                     FROM access_requests ar WHERE ar.id = ?1",
                    params![request_id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, Option<String>>(3)?,
                            r.get::<_, Option<String>>(4)?,
                            r.get::<_, String>(5)?,
                        ))
                    },
                )
                .optional()
                .map_err(|_| AccessRequestError::Db)?;
            let Some((club_id, pilot_id, tail, type_id, new_type_name, status)) = row else {
                return Err(AccessRequestError::NotFound);
            };
            if status != "pending" {
                return Err(AccessRequestError::NotPending);
            }
            let mut resolved_type_id = type_id;
            if resolved_type_id.is_none() {
                let name = new_type_name.as_deref().unwrap_or("").trim();
                if name.is_empty() {
                    return Err(AccessRequestError::MissingType);
                }
                resolved_type_id = if let Some(existing) = conn
                    .query_row(
                        "SELECT id FROM aircraft_types WHERE club_id = ?1 AND code = ?2",
                        params![club_id, name],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(|_| AccessRequestError::Db)?
                {
                    Some(existing)
                } else {
                    let tid = new_id();
                    conn.execute(
                        "INSERT INTO aircraft_types (id, club_id, code) VALUES (?1, ?2, ?3)",
                        params![tid, club_id, name],
                    )
                    .map_err(|_| AccessRequestError::Db)?;
                    conn.execute(
                        "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                         VALUES (?1, ?2, ?3, 'create', 'aircraft_type', ?4, ?5, ?6)",
                        params![new_id(), club_id, admin_id, tid, name, now],
                    )
                    .map_err(|_| AccessRequestError::Db)?;
                    Some(tid)
                };
            }
            let type_id = resolved_type_id.ok_or(AccessRequestError::MissingType)?;
            let aircraft_id = if let Some(aid) = conn
                .query_row(
                    "SELECT id, type_id FROM aircraft WHERE club_id = ?1 AND tail = ?2",
                    params![club_id, tail],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(|_| AccessRequestError::Db)?
            {
                if aid.1 != type_id {
                    return Err(AccessRequestError::TypeMismatch);
                }
                aid.0
            } else {
                let aid = new_id();
                conn.execute(
                    "INSERT INTO aircraft (id, club_id, tail, type_id, active) VALUES (?1, ?2, ?3, ?4, 1)",
                    params![aid, club_id, tail, type_id],
                )
                .map_err(|_| AccessRequestError::Db)?;
                conn.execute(
                    "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                     VALUES (?1, ?2, ?3, 'create', 'aircraft', ?4, ?5, ?6)",
                    params![new_id(), club_id, admin_id, aid, tail, now],
                )
                .map_err(|_| AccessRequestError::Db)?;
                aid
            };
            let checkout_id = new_id();
            conn.execute(
                "INSERT INTO checkouts (id, pilot_id, type_id, aircraft_id, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![checkout_id, pilot_id, type_id, aircraft_id, expires_at],
            )
            .map_err(|_| AccessRequestError::Db)?;
            conn.execute(
                "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                 VALUES (?1, ?2, ?3, 'create', 'checkout', ?4, ?5, ?6)",
                params![new_id(), club_id, admin_id, checkout_id, tail, now],
            )
            .map_err(|_| AccessRequestError::Db)?;
            conn.execute(
                "UPDATE access_requests SET status = 'approved', decided_at = ?1, decided_by_pilot_id = ?2
                 WHERE id = ?3",
                params![now, admin_id, request_id],
            )
            .map_err(|_| AccessRequestError::Db)?;
            conn.execute(
                "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                 VALUES (?1, ?2, ?3, 'approve', 'access_request', ?4, ?5, ?6)",
                params![new_id(), club_id, admin_id, request_id, tail, now],
            )
            .map_err(|_| AccessRequestError::Db)?;
            tx.commit().map_err(|_| AccessRequestError::Db)?;
            Ok(())
        })
    }

    pub fn reject_access_request(
        &self,
        admin_id: &str,
        request_id: &str,
        reason: &str,
        now: i64,
    ) -> Result<(), AccessRequestError> {
        self.with_conn_any(|conn| -> Result<(), AccessRequestError> {
            let row = conn
                .query_row(
                    "SELECT club_id, status FROM access_requests WHERE id = ?1",
                    params![request_id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(|_| AccessRequestError::Db)?;
            let Some((club_id, status)) = row else {
                return Err(AccessRequestError::NotFound);
            };
            if status != "pending" {
                return Err(AccessRequestError::NotPending);
            }
            conn.execute(
                "UPDATE access_requests SET status = 'rejected', reject_reason = ?1,
                    decided_at = ?2, decided_by_pilot_id = ?3 WHERE id = ?4",
                params![reason, now, admin_id, request_id],
            )
            .map_err(|_| AccessRequestError::Db)?;
            conn.execute(
                "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                 VALUES (?1, ?2, ?3, 'reject', 'access_request', ?4, ?5, ?6)",
                params![new_id(), club_id, admin_id, request_id, reason, now],
            )
            .map_err(|_| AccessRequestError::Db)?;
            Ok(())
        })
    }

    pub fn approve_pilot_with_checkout(
        &self,
        admin_id: &str,
        pilot_id: &str,
        checkout: Option<ApprovePilotCheckout>,
    ) -> Result<(), rusqlite::Error> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let (club_id, status): (String, String) = conn.query_row(
                "SELECT club_id, status FROM pilots WHERE id = ?1",
                params![pilot_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if status != "pending" {
                tx.commit()?;
                return Ok(());
            }
            let now = now_ms();
            conn.execute(
                "UPDATE pilots SET status = 'approved' WHERE id = ?1",
                params![pilot_id],
            )?;
            conn.execute(
                "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                 VALUES (?1, ?2, ?3, 'approve', 'pilot', ?4, NULL, ?5)",
                params![new_id(), club_id, admin_id, pilot_id, now],
            )?;
            if let Some(co) = checkout {
                let type_id: String = conn.query_row(
                    "SELECT id FROM aircraft_types WHERE club_id = ?1 AND code = ?2",
                    params![club_id, co.type_code],
                    |r| r.get(0),
                )?;
                let aircraft_id = if let Some(ref tail) = co.tail {
                    Some(conn.query_row(
                        "SELECT id FROM aircraft WHERE club_id = ?1 AND tail = ?2",
                        params![club_id, tail],
                        |r| r.get::<_, String>(0),
                    )?)
                } else {
                    None
                };
                let checkout_id = new_id();
                conn.execute(
                    "INSERT INTO checkouts (id, pilot_id, type_id, aircraft_id, expires_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        checkout_id,
                        pilot_id,
                        type_id,
                        aircraft_id,
                        co.expires_at
                    ],
                )?;
                conn.execute(
                    "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                     VALUES (?1, ?2, ?3, 'create', 'checkout', ?4, ?5, ?6)",
                    params![
                        new_id(),
                        club_id,
                        admin_id,
                        checkout_id,
                        co.type_code,
                        now
                    ],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn add_checkout_typed(
        &self,
        pilot_id: &str,
        type_code: &str,
        tail: Option<&str>,
        expires_at: Option<i64>,
    ) -> Result<(), CheckoutError> {
        self.with_conn_any(|conn| -> Result<(), CheckoutError> {
            let club_id: String = conn.query_row(
                "SELECT club_id FROM pilots WHERE id = ?1",
                params![pilot_id],
                |r| r.get(0),
            )?;
            let (type_id, aircraft_id) = if let Some(tail) = tail {
                let row = conn
                    .query_row(
                        "SELECT a.id, a.type_id FROM aircraft a
                         WHERE a.club_id = ?1 AND a.tail = ?2",
                        params![club_id, tail],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                    )
                    .optional()?;
                let Some((ac_id, ty_id)) = row else {
                    return Err(CheckoutError::UnknownTail);
                };
                (ty_id, Some(ac_id))
            } else {
                let type_id: String = conn
                    .query_row(
                        "SELECT id FROM aircraft_types WHERE club_id = ?1 AND code = ?2",
                        params![club_id, type_code],
                        |r| r.get(0),
                    )
                    .map_err(|_| CheckoutError::UnknownType)?;
                (type_id, None)
            };
            conn.execute(
                "INSERT INTO checkouts (id, pilot_id, type_id, aircraft_id, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![new_id(), pilot_id, type_id, aircraft_id, expires_at],
            )?;
            Ok(())
        })
    }

    pub fn admin_pending_counts(&self, club_id: &str) -> AdminPendingCounts {
        AdminPendingCounts {
            pilots: self.list_pending_pilots().len(),
            access_requests: self.count_pending_access_requests(club_id),
        }
    }

    // from club_scope.rs
pub async fn club_id_for_slug(&self, slug: &str) -> Option<String> {
        let slug = slug.trim();
        if slug.is_empty() {
            return None;
        }
        sqlx::query_scalar("SELECT id FROM clubs WHERE slug = $1")
            .bind(slug)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    pub async fn club_slug(&self, club_id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT slug FROM clubs WHERE id = $1")
            .bind(club_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    pub async fn resolve_club_context(&self, slug: &str) -> Result<ClubContext, ClubScopeError> {
        let Some(club_id) = self.club_id_for_slug(slug).await else {
            return Err(ClubScopeError::NotFound);
        };
        Ok(ClubContext {
            club_id,
            slug: slug.to_string(),
        })
    }

    pub async fn is_superadmin(&self, pilot_id: &str) -> bool {
        sqlx::query_scalar::<_, String>("SELECT global_role FROM pilots WHERE id = $1")
            .bind(pilot_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .map(|r| r == "superadmin")
            .unwrap_or(false)
    }

    pub async fn membership_in_club(
        &self,
        pilot_id: &str,
        club_id: &str,
    ) -> Option<(String, String)> {
        let row = sqlx::query(
            "SELECT role, status FROM club_memberships WHERE pilot_id = $1 AND club_id = $2",
        )
        .bind(pilot_id)
        .bind(club_id)
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        Some((row.try_get(0).ok()?, row.try_get(1).ok()?))
    }

    pub async fn require_club_admin(
        &self,
        pilot_id: &str,
        club_id: &str,
    ) -> Result<(), ClubScopeError> {
        if self.is_superadmin(pilot_id).await {
            return Ok(());
        }
        match self.membership_in_club(pilot_id, club_id).await {
            Some((role, status)) if role == "admin" && status == "approved" => Ok(()),
            _ => Err(ClubScopeError::Forbidden),
        }
    }

    pub async fn ensure_membership_row(
        &self,
        pilot_id: &str,
        club_id: &str,
        role: &str,
        status: &str,
    ) -> Result<(), DbError> {
        use crate::club::auth::new_id;
        sqlx::query(
            "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT(pilot_id, club_id) DO UPDATE SET
               role = excluded.role,
               status = excluded.status",
        )
        .bind(new_id())
        .bind(pilot_id)
        .bind(club_id)
        .bind(role)
        .bind(status)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // from mail_suppress.rs
pub async fn suppress_email(&self, email: &str, reason: &str) -> Result<(), DbError> {
        let email = email.trim().to_ascii_lowercase();
        sqlx::query(
            "INSERT INTO email_suppressions (email, reason, created_at)
             VALUES ($1, $2, $3)
             ON CONFLICT(email) DO UPDATE SET reason = excluded.reason, created_at = excluded.created_at",
        )
        .bind(email)
        .bind(reason)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn email_suppressed(&self, email: &str) -> bool {
        let email = email.trim().to_ascii_lowercase();
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM email_suppressions WHERE email = $1")
            .bind(email)
            .fetch_one(&self.pool)
            .await
            .map(|n| n > 0)
            .unwrap_or(false)
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
    Db(#[allow(dead_code)] sqlx::Error),
}

#[derive(Debug)]
pub enum LoginError {
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartFlightError {
    NoTail,
    NotCleared { tail: String, type_code: String },
    NoCheckin,
    AlreadyActive,
    DeviceMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutError {
    UnknownTail,
    UnknownType,
    Db(#[allow(dead_code)] sqlx::Error),
}

impl From<sqlx::Error> for CheckoutError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckinRecord {
    pub id: String,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckinReject {
    Format,
    ForgedHmac,
    OldStep,
    WrongClub,
    Replay,
}

#[derive(Debug, Serialize)]
pub struct DisplaySessionRow {
    pub id: String,
    pub label: String,
    pub created_at: i64,
    pub revoked: bool,
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
    Db(#[allow(dead_code)] sqlx::Error),
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
pub struct TypeRow {
    pub id: String,
    pub code: String,
}

#[derive(Clone, Serialize)]
pub struct AircraftRow {
    pub id: String,
    pub tail: String,
    pub type_code: String,
    pub active: bool,
}

#[derive(Clone, Serialize)]
pub struct CheckoutRow {
    pub id: String,
    pub pilot_id: String,
    pub pilot_name: String,
    pub pilot_email: String,
    pub type_code: String,
    pub tail: Option<String>,
    pub expires_at: Option<i64>,
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
    use crate::club::access_request::AccessRequestError;
    use crate::club::visibility::{FlightVisibility, PilotRole, PilotStatus, Viewer};

    fn seed(db: &ClubDb) -> (String, String, String, String) {
        let club = db.default_club_id().await.unwrap();
        let type_id = db.upsert_type(&club, "C152").await.unwrap();
        let _ = type_id;
        let ac = db.upsert_aircraft(&club, "N123", "C152", true).await.unwrap();
        let pilot_a = db
            .register(&club, "Alice", "a@test", "L1", "password12")
            .unwrap();
        db.approve_pilot(&pilot_a).await.unwrap();
        let pilot_b = db
            .register(&club, "Bob", "b@test", "L2", "password12")
            .unwrap();
        db.approve_pilot(&pilot_b).await.unwrap();
        db.add_checkout(&pilot_a, "C152", None, None).await.unwrap();
        db.add_checkout(&pilot_b, "C152", None, None).await.unwrap();
        (club, pilot_a, pilot_b, ac)
    }

    fn pilot_device_with_checkin(db: &ClubDb, pilot_id: &str, club_id: &str) -> String {
        let dev = db.create_device(pilot_id, "phone").await.unwrap().0;
        db.issue_open_checkin(pilot_id, &dev, club_id, now_ms())
            .unwrap();
        dev
    }

    #[tokio::test]
    async fn wrong_type_checkout_blocks_tail() {
        let db = ClubDb::open_test().await.unwrap();
        let club = db.default_club_id().await.unwrap();
        db.upsert_type(&club, "DA20").await.unwrap();
        db.upsert_type(&club, "C152").await.unwrap();
        db.upsert_aircraft(&club, "N1", "C152", true).await.unwrap();
        let a = db.register(&club, "A", "a@test", "", "password12").await.unwrap();
        db.approve_pilot(&a).await.unwrap();
        db.add_checkout(&a, "DA20", None, None).await.unwrap();
        let tails = db.me_tails(&a, now_ms()).await.unwrap();
        assert!(tails.is_empty());
    }

    #[tokio::test]
    async fn checkout_expired_blocks_start() {
        let db = ClubDb::open_test().await.unwrap();
        let club = db.default_club_id().await.unwrap();
        db.upsert_type(&club, "C152").await.unwrap();
        db.upsert_aircraft(&club, "N123", "C152", true).await.unwrap();
        let a = db.register(&club, "A", "a@test", "", "password12").await.unwrap();
        db.approve_pilot(&a).await.unwrap();
        let expired = now_ms() - 1000;
        db.add_checkout(&a, "C152", None, Some(expired)).unwrap();
        let tails = db.me_tails(&a, now_ms()).await.unwrap();
        assert!(tails.is_empty());
    }

    #[tokio::test]
    async fn checkout_blocks_inactive_tail() {
        let db = ClubDb::open_test().await.unwrap();
        let club = db.default_club_id().await.unwrap();
        db.upsert_type(&club, "DA20").await.unwrap();
        db.upsert_aircraft(&club, "N999", "DA20", false).await.unwrap();
        let p = db.register(&club, "P", "p@test", "", "password12").await.unwrap();
        db.approve_pilot(&p).await.unwrap();
        db.add_checkout(&p, "DA20", None, None).await.unwrap();
        let tails = db.me_tails(&p, now_ms()).await.unwrap();
        assert!(tails.is_empty());
    }

    #[tokio::test]
    async fn visibility_private_hidden_on_club_map() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, b, _ac) = seed(&db);
        let dev = pilot_device_with_checkin(&db, &a, &club);
        let fid = db
            .start_flight(&a, &dev, "N123", FlightVisibility::Private, false, now_ms())
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
        db.append_fix(&fid, &dev, &fix).await.unwrap();
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

    #[tokio::test]
    async fn admin_tail_filter_does_not_change_pilot_map() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _b, _ac) = seed(&db);
        db.upsert_aircraft(&club, "N456", "C152", true).await.unwrap();
        db.add_checkout(&a, "C152", Some("N456"), None).unwrap();
        let dev = pilot_device_with_checkin(&db, &a, &club);
        let f1 = db
            .start_flight(&a, &dev, "N123", FlightVisibility::Club, false, now_ms())
            .unwrap();
        db.end_flight(&a, &f1, now_ms()).await.unwrap();
        let t2 = now_ms() + 1;
        db.issue_open_checkin(&a, &dev, &club, t2).await.unwrap();
        let f2 = db
            .start_flight(&a, &dev, "N456", FlightVisibility::Club, false, t2)
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
        db.append_fix(&f2, &dev, &fix).await.unwrap();
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

    #[tokio::test]
    async fn pending_pilot_club_map_empty() {
        let db = ClubDb::open_test().await.unwrap();
        let (_club, _a, _b, _ac) = seed(&db);
        let pending = db
            .register(
                &db.default_club_id().await.unwrap(),
                "P",
                "p2@test",
                "",
                "password12",
            )
            .unwrap();
        let viewer = Viewer::Pilot {
            id: pending,
            status: PilotStatus::Pending,
            role: PilotRole::Pilot,
        };
        assert!(db.club_map_for_viewer(&viewer).is_empty());
    }

    #[tokio::test]
    async fn public_map_anonymous_no_name_without_flag() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _b, _ac) = seed(&db);
        let dev = pilot_device_with_checkin(&db, &a, &club);
        let fid = db
            .start_flight(&a, &dev, "N123", FlightVisibility::Public, false, now_ms())
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

    #[tokio::test]
    async fn mid_flight_visibility_change_affects_access() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, b, _ac) = seed(&db);
        let dev = pilot_device_with_checkin(&db, &a, &club);
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

    #[tokio::test]
    async fn checkin_required_to_start_flight() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db);
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        assert!(matches!(
            db.start_flight(&a, &dev, "N123", FlightVisibility::Club, false, now_ms()),
            Err(StartFlightError::NoCheckin)
        ));
        db.issue_open_checkin(&a, &dev, &club, now_ms()).await.unwrap();
        assert!(db
            .start_flight(&a, &dev, "N123", FlightVisibility::Club, false, now_ms())
            .is_ok());
    }

    #[tokio::test]
    async fn checkin_expired_blocks_start() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db);
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        db.issue_open_checkin(&a, &dev, &club, t).await.unwrap();
        db.with_conn(|conn| {
            sqlx::query("UPDATE checkins SET expires_at = $1 WHERE pilot_id = $2").bind(t - 1).bind(a).execute(&self.pool).await?        })
        .unwrap();
        assert!(matches!(
            db.start_flight(&a, &dev, "N123", FlightVisibility::Club, false, t),
            Err(StartFlightError::NoCheckin)
        ));
    }

    #[tokio::test]
    async fn checkin_consumed_on_start_not_reused() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db);
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        db.issue_open_checkin(&a, &dev, &club, t).await.unwrap();
        db.start_flight(&a, &dev, "N123", FlightVisibility::Club, false, t).await
            .unwrap();
        assert!(matches!(
            db.start_flight(&a, &dev, "N123", FlightVisibility::Club, false, t + 1),
            Err(StartFlightError::NoCheckin) | Err(StartFlightError::AlreadyActive)
        ));
    }

    #[tokio::test]
    async fn checkin_token_replay_rejected() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db);
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        let token = db.mint_checkin_qr_token(&club, t).await.unwrap();
        db.submit_checkin(&a, Some(&dev), &token, t).await.unwrap();
        assert_eq!(
            db.submit_checkin(&a, Some(&dev), &token, t),
            Err(CheckinReject::Replay)
        );
    }

    #[tokio::test]
    async fn checkin_old_step_rejected() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db);
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        let step = crate::club::checkin::current_step(t);
        let (secret, _) = db.club_checkin_settings(&club).await.unwrap();
        let old = crate::club::checkin::mint_checkin_token(&secret, &club, step - 2);
        assert_eq!(
            db.submit_checkin(&a, Some(&dev), &old, t),
            Err(CheckinReject::OldStep)
        );
    }

    #[tokio::test]
    async fn checkin_forged_hmac_rejected() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db);
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        let step = crate::club::checkin::current_step(t);
        let bad = format!("v1.{club}.{step}.{}", "0".repeat(64));
        assert_eq!(
            db.submit_checkin(&a, Some(&dev), &bad, t),
            Err(CheckinReject::ForgedHmac)
        );
    }

    #[tokio::test]
    async fn checkin_wrong_club_in_token_rejected() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db);
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        let (secret, _) = db.club_checkin_settings(&club).await.unwrap();
        let step = crate::club::checkin::current_step(t);
        let other_club = "00000000000000000000000000000000";
        let tok = crate::club::checkin::mint_checkin_token(&secret, other_club, step);
        assert_eq!(
            db.submit_checkin(&a, Some(&dev), &tok, t),
            Err(CheckinReject::WrongClub)
        );
    }

    #[tokio::test]
    async fn checkin_happy_path_then_start() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db);
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        let token = db.mint_checkin_qr_token(&club, t).await.unwrap();
        db.submit_checkin(&a, Some(&dev), &token, t).await.unwrap();
        assert!(db
            .start_flight(&a, &dev, "N123", FlightVisibility::Club, false, t)
            .is_ok());
    }

    #[tokio::test]
    async fn checkin_expires_at_uses_club_valid_minutes() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db);
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        db.with_conn(|conn| {
            sqlx::query("UPDATE clubs SET checkin_valid_minutes = 45 WHERE id = $1").bind(club).execute(&self.pool).await?        })
        .unwrap();
        let t = 1_700_000_123_000_i64;
        let token = db.mint_checkin_qr_token(&club, t).await.unwrap();
        let rec = db.submit_checkin(&a, Some(&dev), &token, t).await.unwrap();
        assert_eq!(rec.created_at, t);
        assert_eq!(rec.expires_at, t + 45 * 60_000);
        let stored: (i64, i64) = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT created_at, expires_at FROM checkins WHERE id = $1",
                    params![rec.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
            })
            .unwrap();
        assert_eq!(stored.0 + 45 * 60_000, stored.1);
    }

    #[tokio::test]
    async fn pilot_session_ttl_is_milliseconds() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, _, _, _) = seed(&db);
        let t = now_ms();
        let (_, row) = db.login(&club, "a@test", "password12", t).await.unwrap();
        let exp: i64 = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT expires_at FROM pilot_sessions WHERE pilot_id = $1",
                    params![row.id],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert!(exp >= t + SESSION_TTL_SEC * 1000);
        assert!(exp < t + SESSION_TTL_SEC * 1000 + 60_000);
    }

    #[tokio::test]
    async fn start_flight_rejects_foreign_device() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, b, _) = seed(&db);
        let dev_a = db.create_device(&a, "a-phone").await.unwrap().0;
        let dev_b = db.create_device(&b, "b-phone").await.unwrap().0;
        let t = now_ms();
        let qr = db.mint_checkin_qr_token(&club, t).await.unwrap();
        db.submit_checkin(&b, Some(&dev_b), &qr, t).await.unwrap();
        assert_eq!(
            db.start_flight(&b, &dev_a, "N123", FlightVisibility::Club, false, t),
            Err(StartFlightError::DeviceMismatch)
        );
    }

    #[tokio::test]
    async fn display_map_omits_private_flights() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db);
        let admin = a.clone();
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        let qr = db.mint_checkin_qr_token(&club, t).await.unwrap();
        db.submit_checkin(&a, Some(&dev), &qr, t).await.unwrap();
        let fid = db
            .start_flight(&a, &dev, "N123", FlightVisibility::Private, false, t)
            .unwrap();
        db.append_fix(
            &fid,
            &dev,
            &LiveFix {
                t,
                lat: -33.4,
                lon: -70.6,
                alt: Some(500.0),
                speed: None,
                course: None,
                accuracy: None,
                alt_ref: None,
            },
        )
        .unwrap();
        assert!(db.display_map(&admin).is_empty());
    }

    #[test]
fn empty_club_with_approved_pilot(db: &ClubDb) -> (String, String) {
        let club = db.default_club_id().await.unwrap();
        let p = db.register(&club, "P", "p@test", "L1", "password12").await.unwrap();
        db.approve_pilot(&p).await.unwrap();
        (club, p)
    }

    #[tokio::test]
    async fn access_request_approve_then_start() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, p) = empty_club_with_approved_pilot(&db);
        let admin = db.register(&club, "A", "admin@test", "", "password12").await.unwrap();
        db.approve_pilot(&admin).await.unwrap();
        let dev = db.create_device(&p, "phone").await.unwrap().0;
        db.issue_open_checkin(&p, &dev, &club, now_ms()).await.unwrap();
        let rid = db
            .create_access_request(&p, "CC-ABC", None, Some("C152"), now_ms())
            .unwrap();
        db.approve_access_request(&admin, &rid, None, now_ms())
            .unwrap();
        let tails = db.me_tails(&p, now_ms()).await.unwrap();
        assert!(tails.iter().any(|t| t.tail == "CC-ABC"));
        db.issue_open_checkin(&p, &dev, &club, now_ms()).await.unwrap();
        assert!(db
            .start_flight(
                &p,
                &dev,
                "CC-ABC",
                FlightVisibility::Club,
                false,
                now_ms()
            )
            .is_ok());
    }

    #[tokio::test]
    async fn access_request_reject_blocks_start() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, p) = empty_club_with_approved_pilot(&db);
        let admin = db.register(&club, "A", "admin@test", "", "password12").await.unwrap();
        db.approve_pilot(&admin).await.unwrap();
        let dev = db.create_device(&p, "phone").await.unwrap().0;
        db.issue_open_checkin(&p, &dev, &club, now_ms()).await.unwrap();
        let rid = db
            .create_access_request(&p, "CC-XYZ", None, Some("DA20"), now_ms())
            .unwrap();
        db.reject_access_request(&admin, &rid, "not yet", now_ms())
            .unwrap();
        let list = db.list_pilot_access_requests(&p);
        assert_eq!(list[0].status, "rejected");
        assert_eq!(list[0].reject_reason.as_deref(), Some("not yet"));
        db.upsert_type(&club, "DA20").await.unwrap();
        db.upsert_aircraft(&club, "CC-XYZ", "DA20", true).await.unwrap();
        assert!(matches!(
            db.start_flight(&p, &dev, "CC-XYZ", FlightVisibility::Club, false, now_ms()),
            Err(StartFlightError::NotCleared { .. })
        ));
    }

    #[tokio::test]
    async fn pending_access_request_cannot_start_without_checkout() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, p) = empty_club_with_approved_pilot(&db);
        let dev = db.create_device(&p, "phone").await.unwrap().0;
        db.issue_open_checkin(&p, &dev, &club, now_ms()).await.unwrap();
        db.create_access_request(&p, "CC-NOP", None, Some("T1"), now_ms())
            .unwrap();
        db.upsert_type(&club, "T1").await.unwrap();
        db.upsert_aircraft(&club, "CC-NOP", "T1", true).await.unwrap();
        assert!(matches!(
            db.start_flight(&p, &dev, "CC-NOP", FlightVisibility::Club, false, now_ms()),
            Err(StartFlightError::NotCleared { .. })
        ));
    }

    #[tokio::test]
    async fn add_checkout_unknown_tail_errors() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a) = empty_club_with_approved_pilot(&db);
        db.upsert_type(&club, "C152").await.unwrap();
        assert!(matches!(
            db.add_checkout(&a, "C152", Some("NOPE"), None),
            Err(CheckoutError::UnknownTail)
        ));
    }
}
