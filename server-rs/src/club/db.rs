pub use crate::club::db_auth_pg::{GuestClaimError, MagicConsumeError};

use crate::club::{
    auth::{self, digest_token, new_device_token, new_id},
    checkin::{self, CheckinTokenError},
    database::{self, DbConnectError},
    map::{filter_admin_tails, DeviceFix, TailLivePosition},
    visibility::{FlightVisibility, PilotRole, PilotStatus, Viewer},
};
use crate::live::LiveFix;
use serde::Serialize;
use sqlx::{PgPool, Row};
pub type DbError = sqlx::Error;

#[cfg(test)]
struct TestPgLease {
    #[allow(dead_code)]
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

#[derive(Clone)]
pub struct ClubDb {
    pub(crate) pool: PgPool,
    #[cfg(test)]
    _test_pg_lease: Option<std::sync::Arc<TestPgLease>>,
}

pub const MAX_DEVICES_PER_PILOT: usize = 10;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceRegisterError {
    InvalidInstallId,
    CapExceeded,
    Db,
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum ShareCreateError {
    Db(DbError),
    NotFound,
    NotOwner,
}

#[allow(dead_code)]
pub enum ShareRevokeError {
    Db(DbError),
    NotFound,
    NotOwner,
}
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ApprovePilotCheckout {
    #[serde(rename = "type")]
    pub type_code: String,
    pub tail: Option<String>,
    pub expires_at: Option<i64>,
}
#[derive(serde::Serialize)]
pub struct AdminPendingCounts {
    pub pilots: usize,
    pub access_requests: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct PilotRow {
    #[allow(dead_code)]
    pub id: String,
    pub club_id: String,
    pub name: String,
    pub email: String,
    pub license_no: String,
    pub status: String,
    pub role: String,
    pub locale: String,
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

pub const SHARE_LIFETIME_AFTER_END_MS: i64 = 24 * 3600 * 1000;

#[derive(Debug, Clone)]
pub struct FlightShareRow {
    pub id: String,
    pub flight_id: String,
    pub club_id: String,
    pub created_by: String,
    pub token_hash: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FlightShareListItem {
    pub id: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked: bool,
}

#[derive(Clone, Debug)]
pub struct ResolvedShare {
    pub flight_id: String,
    #[allow(dead_code)]
    pub club_id: String,
    pub tail: String,
    pub status: String,
}

impl ClubDb {
    pub async fn connect() -> Result<Self, DbConnectError> {
        let pool = database::connect_pool().await?;
        database::run_migrations(&pool).await?;
        let db = Self {
            pool,
            #[cfg(test)]
            _test_pg_lease: None,
        };
        db.bootstrap().await?;
        let _ = db.import_device_tokens_from_env().await;
        Ok(db)
    }

    #[cfg(test)]
    pub async fn open_test() -> Result<Self, DbConnectError> {
        let guard = database::acquire_test_pg_integration_lock().await;
        let pool = database::open_test_pool().await?;
        database::run_migrations(&pool).await?;
        let db = Self {
            pool,
            _test_pg_lease: Some(std::sync::Arc::new(TestPgLease { _guard: guard })),
        };
        db.truncate_all().await.map_err(DbConnectError::from)?;
        db.bootstrap().await?;
        Ok(db)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    async fn truncate_all(&self) -> Result<(), DbError> {
        sqlx::query(
            "TRUNCATE TABLE login_attempts, magic_send_log, guest_sessions, magic_login_tokens, \
             webauthn_credentials, email_suppressions, audit_log, access_requests, checkins, \
             device_pair_tokens, flight_devices, fixes, flight_shares, disk_live_session_owners, \
             disk_flight_owners, flights, devices, checkouts, pilot_sessions, \
             admin_map_prefs, display_sessions, club_memberships, pilots, aircraft, aircraft_types, clubs RESTART IDENTITY CASCADE",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn bootstrap(&self) -> Result<(), DbError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM clubs")
            .fetch_one(&self.pool)
            .await?;
        if count == 0 {
            let now = now_ms();
            let club_id = new_id();
            let slug = std::env::var("FLIGHT_CLUB_SLUG").unwrap_or_else(|_| "default".to_string());
            sqlx::query(
                "INSERT INTO clubs (id, name, slug, created_at, checkin_window_hours, latitude, longitude)
                 VALUES ($1, $2, $3, $4, 12, -33.4489, -70.6693)",
            )
            .bind(club_id)
            .bind(std::env::var("FLIGHT_CLUB_NAME").unwrap_or_else(|_| "Flight Club".to_string()))
            .bind(slug)
            .bind(now)
            .execute(&self.pool)
            .await?;
        }
        if let (Ok(email), Ok(password)) = (
            std::env::var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_EMAIL"),
            std::env::var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_PASSWORD"),
        ) {
            if !email.is_empty() && !password.is_empty() {
                let club_id: String = sqlx::query_scalar(
                    "SELECT id FROM clubs ORDER BY created_at ASC, id ASC LIMIT 1",
                )
                .fetch_one(&self.pool)
                .await?;
                let exists: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM pilots WHERE club_id = $1 AND email = $2",
                )
                .bind(&club_id)
                .bind(&email)
                .fetch_one(&self.pool)
                .await?;
                if exists == 0 {
                    let hash =
                        auth::hash_password(&password).map_err(|_| DbError::protocol("hash"))?;
                    sqlx::query(
                        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
                         VALUES ($1, $2, $3, $4, '', 'approved', 'admin', $5, $6)",
                    )
                    .bind(new_id())
                    .bind(&club_id)
                    .bind("Admin")
                    .bind(&email)
                    .bind(&hash)
                    .bind(now_ms())
                    .execute(&self.pool)
                    .await?;
                }
            }
        }
        Ok(())
    }

    pub async fn default_club_id(&self) -> Result<String, DbError> {
        sqlx::query_scalar("SELECT id FROM clubs ORDER BY created_at ASC, id ASC LIMIT 1")
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
        let now = now_ms();
        let member_id = new_id();
        let r = sqlx::query(
            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
             VALUES ($1, $2, $3, $4, $5, 'pending', 'pilot', $6, $7)",
        )
        .bind(&id)
        .bind(club_id)
        .bind(name)
        .bind(email)
        .bind(license_no)
        .bind(&hash)
        .bind(now)
        .execute(&self.pool)
        .await;
        match r {
            Ok(_) => {
                sqlx::query(
                    "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
                     VALUES ($1, $2, $3, 'pilot', 'pending', $4)",
                )
                .bind(member_id)
                .bind(&id)
                .bind(club_id)
                .bind(now)
                .execute(&self.pool)
                .await
                .map_err(RegisterError::Db)?;
                Ok(id)
            }
            Err(e) => {
                if let Some(db_err) = e.as_database_error() {
                    if db_err.code() == Some(std::borrow::Cow::Borrowed("23505")) {
                        return Err(RegisterError::EmailExists);
                    }
                }
                Err(RegisterError::Db(e))
            }
        }
    }

    #[allow(dead_code)]
    pub async fn login(
        &self,
        club_id: &str,
        email: &str,
        password: &str,
        now: i64,
    ) -> Result<(String, PilotRow), LoginError> {
        self.login_password(club_id, email, password, now, "127.0.0.1")
            .await
    }

    pub async fn logout(&self, session_id: &str) -> Result<(), DbError> {
        let session_db_id = auth::digest_token(session_id);
        sqlx::query("DELETE FROM pilot_sessions WHERE id = $1")
            .bind(&session_db_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn pilot_from_session(&self, session_id: &str, now: i64) -> Option<PilotRow> {
        let session_db_id = auth::digest_token(session_id);
        let row = sqlx::query(
            "SELECT p.id, p.club_id, p.name, p.email, p.license_no, p.status, p.role, p.locale
             FROM pilot_sessions s
             JOIN pilots p ON p.id = s.pilot_id
             WHERE s.id = $1 AND s.expires_at > $2",
        )
        .bind(&session_db_id)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        Some(PilotRow {
            id: row.try_get(0).ok()?,
            club_id: row.try_get(1).ok()?,
            name: row.try_get(2).ok()?,
            email: row.try_get(3).ok()?,
            license_no: row.try_get(4).ok()?,
            status: row.try_get(5).ok()?,
            role: row.try_get(6).ok()?,
            locale: row.try_get(7).ok()?,
        })
    }

    pub async fn viewer_from_session(&self, session_id: Option<&str>, now: i64) -> Viewer {
        let Some(session_id) = session_id else {
            return Viewer::Anonymous;
        };
        let Some(pilot) = self.pilot_from_session(session_id, now).await else {
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
        let Some(pilot) = self.pilot_from_session(session_id, now).await else {
            return (Viewer::Anonymous, None);
        };
        let viewer = self.viewer_from_session(Some(session_id), now).await;
        (viewer, Some(pilot))
    }

    #[allow(dead_code)]
    pub async fn approve_pilot(&self, pilot_id: &str) -> Result<(), DbError> {
        sqlx::query("UPDATE pilots SET status = 'approved' WHERE id = $1")
            .bind(pilot_id)
            .execute(&self.pool)
            .await?;
        let club_id: String = sqlx::query_scalar("SELECT club_id FROM pilots WHERE id = $1")
            .bind(pilot_id)
            .fetch_one(&self.pool)
            .await?;
        self.ensure_membership_row(pilot_id, &club_id, "pilot", "approved")
            .await?;
        Ok(())
    }

    pub(crate) async fn checkout_ok(
        pool: &PgPool,
        pilot_id: &str,
        aircraft_id: &str,
        now: i64,
    ) -> Result<bool, DbError> {
        let row = sqlx::query("SELECT type_id, active FROM aircraft WHERE id = $1")
            .bind(aircraft_id)
            .fetch_optional(pool)
            .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        let type_id: String = row.try_get(0)?;
        let active: bool = row.try_get(1)?;
        if !active {
            return Ok(false);
        }
        let aircraft_club: String =
            sqlx::query_scalar("SELECT club_id FROM aircraft WHERE id = $1")
                .bind(aircraft_id)
                .fetch_optional(pool)
                .await?
                .unwrap_or_default();
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM checkouts c
             WHERE c.pilot_id = $1
               AND (c.expires_at IS NULL OR c.expires_at > $2)
               AND (c.club_id IS NULL OR c.club_id = $5)
               AND (
                 (c.aircraft_id = $3) OR
                 (c.aircraft_id IS NULL AND c.type_id = $4)
               )",
        )
        .bind(pilot_id)
        .bind(now)
        .bind(aircraft_id)
        .bind(type_id)
        .bind(&aircraft_club)
        .fetch_one(pool)
        .await?;
        Ok(count > 0)
    }

    async fn effective_club_for_tails(&self, pilot_id: &str, now: i64) -> Result<String, DbError> {
        let home: String = sqlx::query_scalar("SELECT club_id FROM pilots WHERE id = $1")
            .bind(pilot_id)
            .fetch_one(&self.pool)
            .await?;
        let checkin_club: Option<String> = sqlx::query_scalar(
            "SELECT club_id FROM checkins
             WHERE outcome = 'ok' AND pilot_id = $1
               AND expires_at IS NOT NULL AND expires_at > $2
             ORDER BY expires_at DESC LIMIT 1",
        )
        .bind(pilot_id)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        Ok(checkin_club.unwrap_or(home))
    }

    pub async fn me_tails(&self, pilot_id: &str, now: i64) -> Result<Vec<TailInfo>, DbError> {
        let club_id = self.effective_club_for_tails(pilot_id, now).await?;
        let rows = sqlx::query(
            "SELECT a.id, a.tail, t.code
             FROM aircraft a
             JOIN aircraft_types t ON t.id = a.type_id
             WHERE a.active = true AND a.club_id = $1",
        )
        .bind(&club_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for row in rows {
            let id: String = row.try_get(0)?;
            let tail: String = row.try_get(1)?;
            let type_code: String = row.try_get(2)?;
            if Self::checkout_ok(&self.pool, pilot_id, &id, now).await? {
                out.push(TailInfo {
                    id,
                    tail,
                    type_code,
                });
            }
        }
        Ok(out)
    }

    pub async fn start_flight_atomic(
        &self,
        pilot_id: &str,
        device_id: &str,
        tail: &str,
        visibility: FlightVisibility,
        show_pilot_name_public: bool,
        now: i64,
    ) -> Result<String, StartFlightError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| StartFlightError::NoTail)?;
        let result: Result<String, StartFlightError> = {
            let dev_owner: Option<String> =
                sqlx::query_scalar("SELECT pilot_id FROM devices WHERE id = $1")
                    .bind(device_id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|_| StartFlightError::DeviceMismatch)?;
            if dev_owner.as_deref() != Some(pilot_id) {
                Err(StartFlightError::DeviceMismatch)?;
            }
            let checkin_club: String = sqlx::query_scalar(
                "SELECT club_id FROM checkins
                 WHERE outcome = 'ok' AND pilot_id = $1
                   AND expires_at IS NOT NULL AND expires_at > $2
                   AND (device_id IS NULL OR device_id = $3)
                 ORDER BY expires_at DESC LIMIT 1",
            )
            .bind(pilot_id)
            .bind(now)
            .bind(device_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| StartFlightError::NoCheckin)?;
            let aircraft_id: Option<String> = sqlx::query_scalar(
                "SELECT id FROM aircraft WHERE tail = $1 AND club_id = $2 AND active = true",
            )
            .bind(tail)
            .bind(&checkin_club)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| StartFlightError::NoTail)?;
            let aircraft_id = aircraft_id.ok_or(StartFlightError::NoTail)?;
            let type_code: String = sqlx::query_scalar(
                "SELECT t.code FROM aircraft a
                 JOIN aircraft_types t ON t.id = a.type_id
                 WHERE a.id = $1",
            )
            .bind(&aircraft_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| StartFlightError::NoTail)?;
            if !Self::checkout_ok(&self.pool, pilot_id, &aircraft_id, now)
                .await
                .map_err(|_| StartFlightError::NoTail)?
            {
                Err(StartFlightError::NotCleared {
                    tail: tail.to_string(),
                    type_code,
                })?;
            }
            let pilot_active: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM flights WHERE pilot_id = $1 AND status = 'active'",
            )
            .bind(pilot_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| StartFlightError::NoTail)?;
            if pilot_active > 0 {
                Err(StartFlightError::AlreadyActive)?;
            }
            let device_active: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM flights f
                 JOIN flight_devices fd ON fd.flight_id = f.id
                 WHERE fd.device_id = $1 AND f.status = 'active'",
            )
            .bind(device_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| StartFlightError::NoTail)?;
            if device_active > 0 {
                Err(StartFlightError::AlreadyActive)?;
            }
            let club_id = checkin_club;
            let checkins: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM checkins
                 WHERE outcome = 'ok' AND pilot_id = $1 AND club_id = $2
                   AND expires_at IS NOT NULL AND expires_at > $3
                   AND (device_id IS NULL OR device_id = $4)",
            )
            .bind(pilot_id)
            .bind(&club_id)
            .bind(now)
            .bind(device_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| StartFlightError::NoCheckin)?;
            if checkins == 0 {
                Err(StartFlightError::NoCheckin)?;
            }
            let flight_id = new_id();
            sqlx::query(
                "INSERT INTO flights (id, pilot_id, aircraft_id, club_id, status, started_at, visibility, show_pilot_name_public)
                 VALUES ($1, $2, $3, $4, 'active', $5, $6, $7)",
            )
            .bind(&flight_id)
            .bind(pilot_id)
            .bind(&aircraft_id)
            .bind(&club_id)
            .bind(now)
            .bind(visibility.as_str())
            .bind(show_pilot_name_public)
            .execute(&mut *tx)
            .await
            .map_err(|_| StartFlightError::NoTail)?;
            sqlx::query("INSERT INTO flight_devices (flight_id, device_id) VALUES ($1, $2)")
                .bind(&flight_id)
                .bind(device_id)
                .execute(&mut *tx)
                .await
                .map_err(|_| StartFlightError::NoTail)?;
            Ok(flight_id)
        };
        match &result {
            Ok(_) => tx.commit().await.map_err(|_| StartFlightError::NoTail)?,
            Err(_) => {
                let _ = tx.rollback().await;
            }
        }
        result
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
        self.start_flight_atomic(
            pilot_id,
            device_id,
            tail,
            visibility,
            show_pilot_name_public,
            now,
        )
        .await
    }

    pub async fn patch_flight(
        &self,
        pilot_id: &str,
        flight_id: &str,
        visibility: Option<FlightVisibility>,
        show_pilot_name_public: Option<bool>,
    ) -> Result<(), PatchError> {
        let owner = sqlx::query_scalar::<_, String>("SELECT pilot_id FROM flights WHERE id = $1")
            .bind(flight_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| PatchError::NotFound)?;
        let Some(owner) = owner else {
            return Err(PatchError::NotFound);
        };
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

    pub async fn end_flight(
        &self,
        pilot_id: &str,
        flight_id: &str,
        now: i64,
    ) -> Result<(), EndError> {
        let row = sqlx::query("SELECT pilot_id, status FROM flights WHERE id = $1")
            .bind(flight_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| EndError::NotFound)?;
        let Some(row) = row else {
            return Err(EndError::NotFound);
        };
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
        sqlx::query_scalar("SELECT id FROM devices WHERE token_hash = $1")
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
            .await
            .map(|_| ())
    }

    pub async fn append_fixes(
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
            let changed = sqlx::query(
                "INSERT INTO fixes (flight_id, device_id, t_ms, received_at, lat, lon, alt, alt_ref, speed, course, accuracy)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                 ON CONFLICT (flight_id, device_id, t_ms) DO NOTHING",
            )
            .bind(flight_id)
            .bind(device_id)
            .bind(fix.t)
            .bind(now_ms())
            .bind(fix.lat)
            .bind(fix.lon)
            .bind(fix.alt)
            .bind(fix.alt_ref.as_deref())
            .bind(fix.speed)
            .bind(fix.course)
            .bind(fix.accuracy)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            inserted += usize::try_from(changed).unwrap_or(0);
            last_t = fix.t;
        }
        sqlx::query("UPDATE flights SET last_fix_at = $1 WHERE id = $2")
            .bind(last_t)
            .bind(flight_id)
            .execute(&mut *tx)
            .await?;
        if let Ok(pilot_id) =
            sqlx::query_scalar::<_, String>("SELECT pilot_id FROM flights WHERE id = $1")
                .bind(flight_id)
                .fetch_one(&mut *tx)
                .await
        {
            if let Some(last) = fixes.last() {
                let _ = sqlx::query(
                    "UPDATE pilots SET last_latitude = $1, last_longitude = $2, last_position_at = $3 WHERE id = $4",
                )
                .bind(last.lat)
                .bind(last.lon)
                .bind(last.t)
                .bind(pilot_id)
                .execute(&mut *tx)
                .await;
            }
        }
        tx.commit().await?;
        Ok(inserted)
    }

    pub async fn auto_end_idle(&self, idle_ms: i64, now: i64) -> Result<usize, DbError> {
        let cutoff = now - idle_ms;
        let updated = sqlx::query("UPDATE flights SET status = 'ended', ended_at = $1
                 WHERE status = 'active' AND (last_fix_at IS NULL AND started_at < $2 OR last_fix_at < $2)").bind(now).bind(cutoff).execute(&self.pool).await?;
        Ok(updated.rows_affected() as usize)
    }

    pub async fn get_flight(&self, flight_id: &str) -> Option<FlightRow> {
        let row = sqlx::query(
            "SELECT f.id, f.pilot_id, f.aircraft_id, f.status, f.started_at, f.ended_at, f.last_fix_at,
                        f.visibility, f.show_pilot_name_public, a.tail, p.name
                 FROM flights f
                 JOIN aircraft a ON a.id = f.aircraft_id
                 JOIN pilots p ON p.id = f.pilot_id
                 WHERE f.id = $1",
        )
        .bind(flight_id)
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        Some(FlightRow {
            id: row.try_get(0).ok()?,
            pilot_id: row.try_get(1).ok()?,
            aircraft_id: row.try_get(2).ok()?,
            status: row.try_get(3).ok()?,
            started_at: row.try_get(4).ok()?,
            ended_at: row.try_get(5).ok()?,
            last_fix_at: row.try_get(6).ok()?,
            visibility: row.try_get(7).ok()?,
            show_pilot_name_public: row.try_get::<bool, _>(8).ok()?,
            tail: row.try_get(9).ok()?,
            pilot_name: row.try_get(10).ok()?,
        })
    }

    pub async fn flight_visible_to(&self, viewer: &Viewer, flight_id: &str) -> Option<FlightRow> {
        self.flight_visible_to_scoped(viewer, flight_id).await
    }

    #[allow(dead_code)]
    pub async fn mark_email_verified(&self, pilot_id: &str, now: i64) -> Result<(), DbError> {
        sqlx::query("UPDATE pilots SET email_verified_at = $1 WHERE id = $2")
            .bind(now)
            .bind(pilot_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn pilot_email_verified(&self, pilot_id: &str) -> bool {
        sqlx::query_scalar::<_, Option<i64>>("SELECT email_verified_at FROM pilots WHERE id = $1")
            .bind(pilot_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .is_some_and(|t| t.is_some())
    }

    pub async fn fixes_for_flight(&self, flight_id: &str) -> Vec<LiveFix> {
        self.fixes_for_flight_since(flight_id, 0).await
    }

    pub async fn fixes_for_flight_since(&self, flight_id: &str, since_t: i64) -> Vec<LiveFix> {
        let rows = sqlx::query(
            "SELECT t_ms, lat, lon, alt, speed, course, accuracy, alt_ref
             FROM fixes WHERE flight_id = $1 AND t_ms > $2 ORDER BY t_ms ASC",
        )
        .bind(flight_id)
        .bind(since_t)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(LiveFix {
                    t: r.try_get(0).ok()?,
                    lat: r.try_get(1).ok()?,
                    lon: r.try_get(2).ok()?,
                    alt: r.try_get(3).ok()?,
                    alt_ref: r.try_get(7).ok()?,
                    speed: r.try_get(4).ok()?,
                    course: r.try_get(5).ok()?,
                    accuracy: r.try_get(6).ok()?,
                })
            })
            .collect()
    }

    pub(crate) async fn latest_device_fixes(&self) -> Vec<DeviceFix> {
        let rows = sqlx::query(
            "SELECT f.id, a.tail, f.pilot_id, p.name, f.visibility, f.club_id,
                    fx.t_ms, fx.lat, fx.lon, fx.alt, fx.speed, fx.course, fx.accuracy
             FROM flights f
             JOIN aircraft a ON a.id = f.aircraft_id
             JOIN pilots p ON p.id = f.pilot_id
             JOIN fixes fx ON fx.flight_id = f.id
             WHERE f.status = 'active'
               AND fx.t_ms = (
                 SELECT MAX(fx2.t_ms) FROM fixes fx2 WHERE fx2.flight_id = f.id
               )",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(DeviceFix {
                    flight_id: r.try_get(0).ok()?,
                    tail: r.try_get(1).ok()?,
                    pilot_id: r.try_get(2).ok()?,
                    pilot_name: r.try_get(3).ok()?,
                    visibility: r.try_get(4).ok()?,
                    club_id: r.try_get(5).ok()?,
                    fix: LiveFix {
                        t: r.try_get(6).ok()?,
                        lat: r.try_get(7).ok()?,
                        lon: r.try_get(8).ok()?,
                        alt: r.try_get(9).ok()?,
                        alt_ref: None,
                        speed: r.try_get(10).ok()?,
                        course: r.try_get(11).ok()?,
                        accuracy: r.try_get(12).ok()?,
                    },
                })
            })
            .collect()
    }

    pub async fn club_map_for_viewer(&self, viewer: &Viewer) -> Vec<TailLivePosition> {
        self.club_map_for_viewer_scoped(viewer).await
    }

    pub async fn public_map(&self, club_id: &str) -> Vec<TailLivePosition> {
        self.public_map_scoped(club_id).await
    }

    pub async fn admin_map(&self, admin_id: &str) -> Vec<TailLivePosition> {
        let viewer = Viewer::Pilot {
            id: admin_id.to_string(),
            status: PilotStatus::Approved,
            role: PilotRole::Admin,
        };
        let all = self.club_map_for_viewer_scoped(&viewer).await;
        let tails = self.admin_map_tails(admin_id).await;
        filter_admin_tails(all, &tails)
    }

    pub async fn admin_map_tails(&self, admin_id: &str) -> Vec<String> {
        let json: String =
            sqlx::query_scalar("SELECT tails_json FROM admin_map_prefs WHERE pilot_id = $1")
                .bind(admin_id)
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten()
                .unwrap_or_else(|| "[]".to_string());
        serde_json::from_str(&json).unwrap_or_default()
    }

    pub async fn set_admin_map_tails(
        &self,
        admin_id: &str,
        tails: &[String],
    ) -> Result<(), DbError> {
        let json = serde_json::to_string(tails).unwrap_or_else(|_| "[]".to_string());
        sqlx::query(
            "INSERT INTO admin_map_prefs (pilot_id, tails_json) VALUES ($1, $2)
             ON CONFLICT(pilot_id) DO UPDATE SET tails_json = excluded.tails_json",
        )
        .bind(admin_id)
        .bind(json)
        .execute(&self.pool)
        .await?;
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
        sqlx::query(
            "INSERT INTO devices (id, pilot_id, token_hash, label, created_at) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&id)
        .bind(pilot_id)
        .bind(&hash)
        .bind(label)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        sqlx::query("INSERT INTO device_pair_tokens (device_id, token) VALUES ($1, $2)")
            .bind(&id)
            .bind(&token)
            .execute(&self.pool)
            .await?;
        Ok((id, token))
    }

    pub async fn add_checkout(
        &self,
        pilot_id: &str,
        type_code: &str,
        tail: Option<&str>,
        expires_at: Option<i64>,
    ) -> Result<(), CheckoutError> {
        self.add_checkout_typed(pilot_id, type_code, tail, expires_at)
            .await
    }

    pub async fn device_pair_token_for_pilot(
        &self,
        device_id: &str,
        pilot_id: &str,
    ) -> Option<String> {
        sqlx::query_scalar(
            "SELECT dpt.token FROM device_pair_tokens dpt
             JOIN devices d ON d.id = dpt.device_id
             WHERE dpt.device_id = $1 AND d.pilot_id = $2",
        )
        .bind(device_id)
        .bind(pilot_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    pub async fn list_my_devices(&self, pilot_id: &str) -> Vec<DeviceInfo> {
        let rows = sqlx::query("SELECT id, label, created_at FROM devices WHERE pilot_id = $1")
            .bind(pilot_id)
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(DeviceInfo {
                    id: r.try_get(0).ok()?,
                    label: r.try_get(1).ok()?,
                    created_at: r.try_get(2).ok()?,
                })
            })
            .collect()
    }

    pub async fn list_my_flights(&self, pilot_id: &str) -> Vec<FlightSummary> {
        let rows = sqlx::query(
            "SELECT f.id, a.tail, f.status, f.started_at, f.ended_at, f.visibility, f.show_pilot_name_public
             FROM flights f JOIN aircraft a ON a.id = f.aircraft_id
             WHERE f.pilot_id = $1 ORDER BY f.started_at DESC",
        )
        .bind(pilot_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(FlightSummary {
                    id: r.try_get(0).ok()?,
                    tail: r.try_get(1).ok()?,
                    status: r.try_get(2).ok()?,
                    started_at: r.try_get(3).ok()?,
                    ended_at: r.try_get(4).ok()?,
                    visibility: r.try_get(5).ok()?,
                    show_pilot_name_public: r.try_get::<bool, _>(6).ok()?,
                })
            })
            .collect()
    }

    pub async fn delete_my_flight(
        &self,
        pilot_id: &str,
        flight_id: &str,
    ) -> Result<(), DeleteError> {
        let owner: String = match sqlx::query_scalar("SELECT pilot_id FROM flights WHERE id = $1")
            .bind(flight_id)
            .fetch_optional(&self.pool)
            .await
        {
            Ok(Some(o)) => o,
            Ok(None) => return Err(DeleteError::NotFound),
            Err(e) => return Err(DeleteError::Db(e)),
        };
        if owner != pilot_id {
            return Err(DeleteError::Forbidden);
        }
        sqlx::query("DELETE FROM flights WHERE id = $1")
            .bind(flight_id)
            .execute(&self.pool)
            .await
            .map_err(DeleteError::Db)?;
        Ok(())
    }

    pub async fn upsert_type(&self, club_id: &str, code: &str) -> Result<String, DbError> {
        if let Some(id) = sqlx::query_scalar::<_, String>(
            "SELECT id FROM aircraft_types WHERE club_id = $1 AND code = $2",
        )
        .bind(club_id)
        .bind(code)
        .fetch_optional(&self.pool)
        .await?
        {
            return Ok(id);
        }
        let id = new_id();
        sqlx::query("INSERT INTO aircraft_types (id, club_id, code) VALUES ($1, $2, $3)")
            .bind(&id)
            .bind(club_id)
            .bind(code)
            .execute(&self.pool)
            .await?;
        Ok(id)
    }

    pub async fn upsert_aircraft(
        &self,
        club_id: &str,
        tail: &str,
        type_code: &str,
        active: bool,
    ) -> Result<String, DbError> {
        let type_id: String =
            sqlx::query_scalar("SELECT id FROM aircraft_types WHERE club_id = $1 AND code = $2")
                .bind(club_id)
                .bind(type_code)
                .fetch_one(&self.pool)
                .await?;
        if let Some(id) = sqlx::query_scalar::<_, String>(
            "SELECT id FROM aircraft WHERE club_id = $1 AND tail = $2",
        )
        .bind(club_id)
        .bind(tail)
        .fetch_optional(&self.pool)
        .await?
        {
            sqlx::query("UPDATE aircraft SET type_id = $1, active = $2 WHERE id = $3")
                .bind(&type_id)
                .bind(active)
                .bind(&id)
                .execute(&self.pool)
                .await?;
            return Ok(id);
        }
        let id = new_id();
        sqlx::query(
            "INSERT INTO aircraft (id, club_id, tail, type_id, active) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&id)
        .bind(club_id)
        .bind(tail)
        .bind(&type_id)
        .bind(active)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn list_pending_pilots(&self, club_id: &str) -> Vec<PilotRow> {
        let rows = sqlx::query(
            "SELECT id, club_id, name, email, license_no, status, role, locale
             FROM pilots WHERE status = 'pending' AND club_id = $1",
        )
        .bind(club_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(PilotRow {
                    id: r.try_get(0).ok()?,
                    club_id: r.try_get(1).ok()?,
                    name: r.try_get(2).ok()?,
                    email: r.try_get(3).ok()?,
                    license_no: r.try_get(4).ok()?,
                    status: r.try_get(5).ok()?,
                    role: r.try_get(6).ok()?,
                    locale: r.try_get(7).ok()?,
                })
            })
            .collect()
    }

    pub async fn list_types(&self, club_id: &str) -> Vec<TypeRow> {
        let rows =
            sqlx::query("SELECT id, code FROM aircraft_types WHERE club_id = $1 ORDER BY code")
                .bind(club_id)
                .fetch_all(&self.pool)
                .await
                .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(TypeRow {
                    id: r.try_get(0).ok()?,
                    code: r.try_get(1).ok()?,
                })
            })
            .collect()
    }

    pub async fn list_aircraft(&self, club_id: &str) -> Vec<AircraftRow> {
        let rows = sqlx::query(
            "SELECT a.id, a.tail, t.code, a.active
             FROM aircraft a JOIN aircraft_types t ON t.id = a.type_id
             WHERE a.club_id = $1 ORDER BY a.tail",
        )
        .bind(club_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(AircraftRow {
                    id: r.try_get(0).ok()?,
                    tail: r.try_get(1).ok()?,
                    type_code: r.try_get(2).ok()?,
                    active: r.try_get::<bool, _>(3).ok()?,
                })
            })
            .collect()
    }

    pub async fn list_all_pilots(&self, club_id: &str) -> Vec<PilotRow> {
        let rows = sqlx::query(
            "SELECT id, club_id, name, email, license_no, status, role, locale
             FROM pilots WHERE club_id = $1 ORDER BY email",
        )
        .bind(club_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(PilotRow {
                    id: r.try_get(0).ok()?,
                    club_id: r.try_get(1).ok()?,
                    name: r.try_get(2).ok()?,
                    email: r.try_get(3).ok()?,
                    license_no: r.try_get(4).ok()?,
                    status: r.try_get(5).ok()?,
                    role: r.try_get(6).ok()?,
                    locale: r.try_get(7).ok()?,
                })
            })
            .collect()
    }

    pub async fn list_checkouts(&self, club_id: &str) -> Vec<CheckoutRow> {
        let rows = sqlx::query(
            "SELECT c.id, c.pilot_id, p.name, p.email, t.code, a.tail, c.expires_at
             FROM checkouts c
             JOIN pilots p ON p.id = c.pilot_id
             JOIN aircraft_types t ON t.id = c.type_id
             LEFT JOIN aircraft a ON a.id = c.aircraft_id
             WHERE t.club_id = $1
             ORDER BY p.email, t.code",
        )
        .bind(club_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(CheckoutRow {
                    id: r.try_get(0).ok()?,
                    pilot_id: r.try_get(1).ok()?,
                    pilot_name: r.try_get(2).ok()?,
                    pilot_email: r.try_get(3).ok()?,
                    type_code: r.try_get(4).ok()?,
                    tail: r.try_get(5).ok()?,
                    expires_at: r.try_get(6).ok()?,
                })
            })
            .collect()
    }

    pub async fn pilot_club_id(&self, pilot_id: &str) -> Result<String, DbError> {
        sqlx::query_scalar("SELECT club_id FROM pilots WHERE id = $1")
            .bind(pilot_id)
            .fetch_one(&self.pool)
            .await
    }

    pub async fn club_checkin_settings(&self, club_id: &str) -> Result<(String, i64), DbError> {
        let row = sqlx::query(
            "SELECT checkin_hmac_secret, checkin_valid_minutes FROM clubs WHERE id = $1",
        )
        .bind(club_id)
        .fetch_one(&self.pool)
        .await?;
        let secret: String = row.try_get(0)?;
        let minutes: i32 = row.try_get(1)?;
        let minutes = i64::from(minutes);
        let secret = if secret.is_empty() {
            self.ensure_checkin_secret(club_id).await?
        } else {
            secret
        };
        Ok((secret, minutes.max(1)))
    }

    pub async fn ensure_checkin_secret(&self, club_id: &str) -> Result<String, DbError> {
        if let Ok(env) = std::env::var("FLIGHT_CLUB_CHECKIN_SECRET") {
            if !env.is_empty() {
                sqlx::query("UPDATE clubs SET checkin_hmac_secret = $1 WHERE id = $2")
                    .bind(&env)
                    .bind(club_id)
                    .execute(&self.pool)
                    .await?;
                return Ok(env);
            }
        }
        let secret = new_device_token();
        sqlx::query("UPDATE clubs SET checkin_hmac_secret = $1 WHERE id = $2")
            .bind(&secret)
            .bind(club_id)
            .execute(&self.pool)
            .await?;
        Ok(secret)
    }

    #[allow(clippy::too_many_arguments)]
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
        sqlx::query(
            "INSERT INTO checkins (id, club_id, pilot_id, device_id, time_step, token_digest, created_at, expires_at, outcome, fail_reason)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(&id)
        .bind(club_id)
        .bind(pilot_id)
        .bind(device_id)
        .bind(time_step)
        .bind(token_digest)
        .bind(now)
        .bind(expires_at)
        .bind(outcome)
        .bind(fail_reason)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn mint_checkin_qr_token(&self, club_id: &str, now: i64) -> Result<String, DbError> {
        let (secret, _) = self.club_checkin_settings(club_id).await?;
        let step = checkin::current_step(now);
        Ok(checkin::mint_checkin_token(&secret, club_id, step))
    }

    async fn checkin_replay(
        &self,
        pilot_id: &str,
        device_id: Option<&str>,
        token_digest: &str,
    ) -> Result<bool, DbError> {
        let count: i64 = if let Some(dev) = device_id {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM checkins WHERE outcome = 'ok' AND pilot_id = $1 AND device_id = $2 AND token_digest = $3",
            )
            .bind(pilot_id)
            .bind(dev)
            .bind(token_digest)
            .fetch_one(&self.pool)
            .await?
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM checkins WHERE outcome = 'ok' AND pilot_id = $1 AND device_id IS NULL AND token_digest = $2",
            )
            .bind(pilot_id)
            .bind(token_digest)
            .fetch_one(&self.pool)
            .await?
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
            .await
            .map_err(|_| CheckinReject::WrongClub)?;
        let (secret, valid_min) = self
            .club_checkin_settings(&pilot_club)
            .await
            .map_err(|_| CheckinReject::ForgedHmac)?;
        let digest = checkin::token_digest(token);
        macro_rules! fail_checkin {
            ($reason:expr, $err:expr) => {{
                let _ = self
                    .insert_checkin_row(
                        &pilot_club,
                        Some(pilot_id),
                        device_id,
                        0,
                        &digest,
                        now,
                        None,
                        "fail",
                        Some($reason),
                    )
                    .await;
                return Err($err);
            }};
        }
        let (club_id, step) = match checkin::verify_checkin_token(&secret, token, now) {
            Ok(v) => v,
            Err(CheckinTokenError::Format) => fail_checkin!("format", CheckinReject::Format),
            Err(CheckinTokenError::Forged) => {
                fail_checkin!("forged_hmac", CheckinReject::ForgedHmac)
            }
            Err(CheckinTokenError::OldStep) => fail_checkin!("old_step", CheckinReject::OldStep),
        };
        if club_id != pilot_club {
            fail_checkin!("wrong_club", CheckinReject::WrongClub);
        }
        if self
            .checkin_replay(pilot_id, device_id, &digest)
            .await
            .unwrap_or(false)
        {
            fail_checkin!("replay", CheckinReject::Replay);
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
            .await
            .map_err(|_| CheckinReject::ForgedHmac)?;
        Ok(CheckinRecord {
            id,
            created_at: now,
            expires_at: expires,
        })
    }

    #[allow(dead_code)]
    pub async fn consume_checkin(
        &self,
        pilot_id: &str,
        device_id: &str,
        club_id: &str,
        now: i64,
    ) -> Result<bool, DbError> {
        let checkin_id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM checkins
             WHERE outcome = 'ok' AND pilot_id = $1 AND club_id = $2
               AND consumed_at IS NULL AND expires_at IS NOT NULL AND expires_at > $3
               AND (device_id IS NULL OR device_id = $4)
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(pilot_id)
        .bind(club_id)
        .bind(now)
        .bind(device_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(id) = checkin_id else {
            return Ok(false);
        };
        sqlx::query("UPDATE checkins SET consumed_at = $1 WHERE id = $2 AND consumed_at IS NULL")
            .bind(now)
            .bind(&id)
            .execute(&self.pool)
            .await?;
        Ok(true)
    }

    pub async fn issue_open_checkin(
        &self,
        pilot_id: &str,
        device_id: &str,
        club_id: &str,
        now: i64,
    ) -> Result<(), DbError> {
        let (_, valid_min) = self.club_checkin_settings(club_id).await?;
        let expires = now + valid_min * 60_000;
        let digest = checkin::token_digest(&format!("internal:{pilot_id}:{now}"));
        let _ = self
            .insert_checkin_row(
                club_id,
                Some(pilot_id),
                Some(device_id),
                checkin::current_step(now),
                &digest,
                now,
                Some(expires),
                "ok",
                None,
            )
            .await?;
        Ok(())
    }

    pub async fn create_display_session(
        &self,
        admin_id: &str,
        label: &str,
    ) -> Result<(String, String), DbError> {
        let club_id = self.pilot_club_id(admin_id).await?;
        let token = new_device_token();
        let hash = digest_token(&token);
        let id = new_id();
        let now = now_ms();
        let expires_at = now + 90 * 24 * 3600 * 1000;
        sqlx::query(
            "INSERT INTO display_sessions (id, club_id, created_by_pilot_id, token_hash, label, created_at, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&id)
        .bind(&club_id)
        .bind(admin_id)
        .bind(&hash)
        .bind(label)
        .bind(now)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok((id, token))
    }

    pub async fn revoke_display_session(
        &self,
        admin_id: &str,
        session_id: &str,
    ) -> Result<bool, DbError> {
        let club_id = self.pilot_club_id(admin_id).await?;
        let updated = sqlx::query(
            "UPDATE display_sessions SET revoked_at = $1
             WHERE id = $2 AND club_id = $3 AND created_by_pilot_id = $4 AND revoked_at IS NULL",
        )
        .bind(now_ms())
        .bind(session_id)
        .bind(&club_id)
        .bind(admin_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() > 0)
    }

    pub async fn resolve_display_session(&self, token: &str) -> Option<(String, String, String)> {
        let hash = digest_token(token);
        let now = now_ms();
        let row = sqlx::query(
            "SELECT id, club_id, created_by_pilot_id, expires_at FROM display_sessions
             WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(hash)
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        let expires_at: Option<i64> = row.try_get(3).ok().flatten();
        if expires_at.is_some_and(|exp| now > exp) {
            return None;
        }
        Some((
            row.try_get(0).ok()?,
            row.try_get(1).ok()?,
            row.try_get(2).ok()?,
        ))
    }

    pub async fn list_display_sessions(&self, admin_id: &str) -> Vec<DisplaySessionRow> {
        let rows = sqlx::query(
            "SELECT id, label, created_at, revoked_at FROM display_sessions
             WHERE created_by_pilot_id = $1 ORDER BY created_at DESC",
        )
        .bind(admin_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                let revoked_at: Option<i64> = r.try_get(3).ok()?;
                Some(DisplaySessionRow {
                    id: r.try_get(0).ok()?,
                    label: r.try_get(1).ok()?,
                    created_at: r.try_get(2).ok()?,
                    revoked: revoked_at.is_some(),
                })
            })
            .collect()
    }

    pub async fn display_map(
        &self,
        display_club_id: &str,
        admin_creator_id: &str,
    ) -> Vec<TailLivePosition> {
        self.display_map_scoped(display_club_id, admin_creator_id)
            .await
    }

    pub async fn seed_demo_if_empty(&self) -> Result<(), DbError> {
        let club_id = self.default_club_id().await?;
        let exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM pilots WHERE email = 'admin@demo.test'")
                .fetch_one(&self.pool)
                .await?;
        if exists > 0 {
            return Ok(());
        }
        let admin_id = new_id();
        let pilot_id = new_id();
        let pending_id = new_id();
        let hash = auth::hash_password("password12").map_err(|_| DbError::protocol("hash"))?;
        let t = now_ms();
        sqlx::query(
            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at) VALUES
             ($1, $4, 'Demo Admin', 'admin@demo.test', 'ADM-1', 'approved', 'admin', $5, $6),
             ($2, $4, 'Demo Pilot', 'pilot@demo.test', 'P-1', 'approved', 'pilot', $5, $6),
             ($3, $4, 'Demo Pending', 'pending@demo.test', 'P-2', 'pending', 'pilot', $5, $6)",
        )
        .bind(&admin_id)
        .bind(&pilot_id)
        .bind(&pending_id)
        .bind(&club_id)
        .bind(&hash)
        .bind(t)
        .execute(&self.pool)
        .await?;
        for code in ["DA20-C1", "C152"] {
            self.upsert_type(&club_id, code).await?;
        }
        for tail in ["N101", "N202", "N303"] {
            self.upsert_aircraft(&club_id, tail, "C152", true).await?;
        }
        self.add_checkout(&pilot_id, "C152", None, None)
            .await
            .map_err(|e| DbError::protocol(format!("checkout: {e:?}")))?;
        self.add_checkout(&admin_id, "C152", None, None)
            .await
            .map_err(|e| DbError::protocol(format!("checkout: {e:?}")))?;
        let (dev_pilot, _) = self.create_device(&pilot_id, "Pilot phone").await?;
        let (dev_admin, _) = self.create_device(&admin_id, "Admin tablet").await?;
        self.issue_open_checkin(&pilot_id, &dev_pilot, &club_id, t)
            .await?;
        self.issue_open_checkin(&admin_id, &dev_admin, &club_id, t)
            .await?;
        let seed_flight_err =
            |e: StartFlightError| DbError::protocol(format!("seed_demo flight: {e:?}"));
        let pub_id = self
            .start_flight(
                &pilot_id,
                &dev_pilot,
                "N101",
                FlightVisibility::Public,
                true,
                t,
            )
            .await
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
            .await
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
        self.append_fix(&pub_id, &dev_pilot, &fix(100, -33.40))
            .await?;
        self.append_fix(&pub_id, &dev_pilot, &fix(200, -33.39))
            .await?;
        self.append_fix(&club_f, &dev_admin, &fix(150, -33.38))
            .await?;
        self.append_fix(&club_f, &dev_admin, &fix(250, -33.37))
            .await?;
        Ok(())
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
    Throttled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartFlightError {
    NoTail,
    NotCleared { tail: String, type_code: String },
    NoCheckin,
    AlreadyActive,
    DeviceMismatch,
}

#[derive(Debug)]
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
    use crate::club::auth::SESSION_TTL_SEC;
    use crate::club::visibility::{FlightVisibility, PilotRole, PilotStatus, Viewer};
    use serial_test::serial;

    async fn seed(db: &ClubDb) -> (String, String, String, String) {
        let club = db.default_club_id().await.unwrap();
        let type_id = db.upsert_type(&club, "C152").await.unwrap();
        let _ = type_id;
        let ac = db
            .upsert_aircraft(&club, "N123", "C152", true)
            .await
            .unwrap();
        let pilot_a = db
            .register(&club, "Alice", "a@test", "L1", "password12")
            .await
            .unwrap();
        db.approve_pilot(&pilot_a).await.unwrap();
        let pilot_b = db
            .register(&club, "Bob", "b@test", "L2", "password12")
            .await
            .unwrap();
        db.approve_pilot(&pilot_b).await.unwrap();
        db.add_checkout(&pilot_a, "C152", None, None).await.unwrap();
        db.add_checkout(&pilot_b, "C152", None, None).await.unwrap();
        (club, pilot_a, pilot_b, ac)
    }

    async fn pilot_device_with_checkin(db: &ClubDb, pilot_id: &str, club_id: &str) -> String {
        let dev = db.create_device(pilot_id, "phone").await.unwrap().0;
        db.issue_open_checkin(pilot_id, &dev, club_id, now_ms())
            .await
            .unwrap();
        dev
    }

    #[serial]
    #[serial]
    #[tokio::test]
    async fn wrong_type_checkout_blocks_tail() {
        let db = ClubDb::open_test().await.unwrap();
        let club = db.default_club_id().await.unwrap();
        db.upsert_type(&club, "DA20").await.unwrap();
        db.upsert_type(&club, "C152").await.unwrap();
        db.upsert_aircraft(&club, "N1", "C152", true).await.unwrap();
        let a = db
            .register(&club, "A", "a@test", "", "password12")
            .await
            .unwrap();
        db.approve_pilot(&a).await.unwrap();
        db.add_checkout(&a, "DA20", None, None).await.unwrap();
        let tails = db.me_tails(&a, now_ms()).await.unwrap();
        assert!(tails.is_empty());
    }

    #[serial]
    #[tokio::test]
    async fn checkout_expired_blocks_start() {
        let db = ClubDb::open_test().await.unwrap();
        let club = db.default_club_id().await.unwrap();
        db.upsert_type(&club, "C152").await.unwrap();
        db.upsert_aircraft(&club, "N123", "C152", true)
            .await
            .unwrap();
        let a = db
            .register(&club, "A", "a@test", "", "password12")
            .await
            .unwrap();
        db.approve_pilot(&a).await.unwrap();
        let expired = now_ms() - 1000;
        db.add_checkout(&a, "C152", None, Some(expired))
            .await
            .unwrap();
        let tails = db.me_tails(&a, now_ms()).await.unwrap();
        assert!(tails.is_empty());
    }

    #[serial]
    #[tokio::test]
    async fn checkout_blocks_inactive_tail() {
        let db = ClubDb::open_test().await.unwrap();
        let club = db.default_club_id().await.unwrap();
        db.upsert_type(&club, "DA20").await.unwrap();
        db.upsert_aircraft(&club, "N999", "DA20", false)
            .await
            .unwrap();
        let p = db
            .register(&club, "P", "p@test", "", "password12")
            .await
            .unwrap();
        db.approve_pilot(&p).await.unwrap();
        db.add_checkout(&p, "DA20", None, None).await.unwrap();
        let tails = db.me_tails(&p, now_ms()).await.unwrap();
        assert!(tails.is_empty());
    }

    #[serial]
    #[tokio::test]
    async fn visibility_private_hidden_on_club_map() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, b, _ac) = seed(&db).await;
        let dev = pilot_device_with_checkin(&db, &a, &club).await;
        let fid = db
            .start_flight(&a, &dev, "N123", FlightVisibility::Private, false, now_ms())
            .await
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
        let map = db.club_map_for_viewer(&viewer_b).await;
        assert!(map.is_empty());
        let viewer_a = Viewer::Pilot {
            id: a.clone(),
            status: PilotStatus::Approved,
            role: PilotRole::Pilot,
        };
        assert_eq!(db.club_map_for_viewer(&viewer_a).await.len(), 1);
    }

    #[serial]
    #[tokio::test]
    async fn admin_tail_filter_does_not_change_pilot_map() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _b, _ac) = seed(&db).await;
        db.upsert_aircraft(&club, "N456", "C152", true)
            .await
            .unwrap();
        db.add_checkout(&a, "C152", Some("N456"), None)
            .await
            .unwrap();
        let dev = pilot_device_with_checkin(&db, &a, &club).await;
        let f1 = db
            .start_flight(&a, &dev, "N123", FlightVisibility::Club, false, now_ms())
            .await
            .unwrap();
        db.end_flight(&a, &f1, now_ms()).await.unwrap();
        let t2 = now_ms() + 1;
        db.issue_open_checkin(&a, &dev, &club, t2).await.unwrap();
        let f2 = db
            .start_flight(&a, &dev, "N456", FlightVisibility::Club, false, t2)
            .await
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
        db.set_admin_map_tails(&a, &["N123".to_string()])
            .await
            .unwrap();
        let admin_map = db.admin_map(&a).await;
        assert!(admin_map.is_empty());
        let pilot_map = db
            .club_map_for_viewer(&Viewer::Pilot {
                id: a.clone(),
                status: PilotStatus::Approved,
                role: PilotRole::Pilot,
            })
            .await;
        assert_eq!(pilot_map.len(), 1);
        assert_eq!(pilot_map[0].tail, "N456");
    }

    #[serial]
    #[tokio::test]
    async fn pending_pilot_club_map_empty() {
        let db = ClubDb::open_test().await.unwrap();
        let (_club, _a, _b, _ac) = seed(&db).await;
        let pending = db
            .register(
                &db.default_club_id().await.unwrap(),
                "P",
                "p2@test",
                "",
                "password12",
            )
            .await
            .unwrap();
        let viewer = Viewer::Pilot {
            id: pending,
            status: PilotStatus::Pending,
            role: PilotRole::Pilot,
        };
        assert!(db.club_map_for_viewer(&viewer).await.is_empty());
    }

    #[serial]
    #[tokio::test]
    async fn public_map_anonymous_no_name_without_flag() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _b, _ac) = seed(&db).await;
        let dev = pilot_device_with_checkin(&db, &a, &club).await;
        let fid = db
            .start_flight(&a, &dev, "N123", FlightVisibility::Public, false, now_ms())
            .await
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
        .await
        .unwrap();
        let map = db.public_map(&club).await;
        assert_eq!(map.len(), 1);
        assert!(map[0].pilot_name.is_none());
    }

    #[serial]
    #[tokio::test]
    async fn club_map_hides_other_pilot_name_without_opt_in() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, b, _ac) = seed(&db).await;
        let dev = pilot_device_with_checkin(&db, &a, &club).await;
        let fid = db
            .start_flight(&a, &dev, "N123", FlightVisibility::Club, false, now_ms())
            .await
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
        .await
        .unwrap();
        let viewer_b = Viewer::Pilot {
            id: b.clone(),
            status: PilotStatus::Approved,
            role: PilotRole::Pilot,
        };
        let map_b = db.club_map_for_viewer(&viewer_b).await;
        assert_eq!(map_b.len(), 1);
        assert!(map_b[0].pilot_name.is_none());
        let viewer_a = Viewer::Pilot {
            id: a.clone(),
            status: PilotStatus::Approved,
            role: PilotRole::Pilot,
        };
        let map_a = db.club_map_for_viewer(&viewer_a).await;
        assert_eq!(map_a[0].pilot_name.as_deref(), Some("Alice"));
    }

    #[serial]
    #[tokio::test]
    async fn mid_flight_visibility_change_affects_access() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, b, _ac) = seed(&db).await;
        let dev = pilot_device_with_checkin(&db, &a, &club).await;
        let fid = db
            .start_flight(&a, &dev, "N123", FlightVisibility::Club, false, now_ms())
            .await
            .unwrap();
        let viewer_b = Viewer::Pilot {
            id: b.clone(),
            status: PilotStatus::Approved,
            role: PilotRole::Pilot,
        };
        assert!(db.flight_visible_to(&viewer_b, &fid).await.is_some());
        db.patch_flight(&a, &fid, Some(FlightVisibility::Private), None)
            .await
            .unwrap();
        assert!(db.flight_visible_to(&viewer_b, &fid).await.is_none());
    }

    #[serial]
    #[tokio::test]
    async fn checkin_required_to_start_flight() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db).await;
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        assert!(matches!(
            db.start_flight(&a, &dev, "N123", FlightVisibility::Club, false, now_ms())
                .await,
            Err(StartFlightError::NoCheckin)
        ));
        db.issue_open_checkin(&a, &dev, &club, now_ms())
            .await
            .unwrap();
        assert!(db
            .start_flight(&a, &dev, "N123", FlightVisibility::Club, false, now_ms())
            .await
            .is_ok());
    }

    #[serial]
    #[tokio::test]
    async fn checkin_expired_blocks_start() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db).await;
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        db.issue_open_checkin(&a, &dev, &club, t).await.unwrap();
        sqlx::query("UPDATE checkins SET expires_at = $1 WHERE pilot_id = $2")
            .bind(t - 1)
            .bind(&a)
            .execute(&db.pool)
            .await
            .unwrap();
        assert!(matches!(
            db.start_flight(&a, &dev, "N123", FlightVisibility::Club, false, t)
                .await,
            Err(StartFlightError::NoCheckin)
        ));
    }

    #[serial]
    #[tokio::test]
    async fn checkin_consumed_on_start_not_reused() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db).await;
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        db.issue_open_checkin(&a, &dev, &club, t).await.unwrap();
        db.start_flight(&a, &dev, "N123", FlightVisibility::Club, false, t)
            .await
            .unwrap();
        assert!(matches!(
            db.start_flight(&a, &dev, "N123", FlightVisibility::Club, false, t + 1)
                .await,
            Err(StartFlightError::NoCheckin) | Err(StartFlightError::AlreadyActive)
        ));
    }

    #[serial]
    #[tokio::test]
    async fn checkin_token_replay_rejected() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db).await;
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        let token = db.mint_checkin_qr_token(&club, t).await.unwrap();
        db.submit_checkin(&a, Some(&dev), &token, t).await.unwrap();
        assert_eq!(
            db.submit_checkin(&a, Some(&dev), &token, t).await,
            Err(CheckinReject::Replay)
        );
    }

    #[serial]
    #[tokio::test]
    async fn checkin_old_step_rejected() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db).await;
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        let step = crate::club::checkin::current_step(t);
        let (secret, _) = db.club_checkin_settings(&club).await.unwrap();
        let old = crate::club::checkin::mint_checkin_token(&secret, &club, step - 2);
        assert_eq!(
            db.submit_checkin(&a, Some(&dev), &old, t).await,
            Err(CheckinReject::OldStep)
        );
    }

    #[serial]
    #[tokio::test]
    async fn checkin_forged_hmac_rejected() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db).await;
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        let step = crate::club::checkin::current_step(t);
        let bad = format!("v1.{club}.{step}.{}", "0".repeat(64));
        assert_eq!(
            db.submit_checkin(&a, Some(&dev), &bad, t).await,
            Err(CheckinReject::ForgedHmac)
        );
    }

    #[serial]
    #[tokio::test]
    async fn checkin_wrong_club_in_token_rejected() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db).await;
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        let (secret, _) = db.club_checkin_settings(&club).await.unwrap();
        let step = crate::club::checkin::current_step(t);
        let other_club = "00000000000000000000000000000000";
        let tok = crate::club::checkin::mint_checkin_token(&secret, other_club, step);
        assert_eq!(
            db.submit_checkin(&a, Some(&dev), &tok, t).await,
            Err(CheckinReject::WrongClub)
        );
    }

    #[serial]
    #[tokio::test]
    async fn checkin_happy_path_then_start() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db).await;
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        let token = db.mint_checkin_qr_token(&club, t).await.unwrap();
        db.submit_checkin(&a, Some(&dev), &token, t).await.unwrap();
        assert!(db
            .start_flight(&a, &dev, "N123", FlightVisibility::Club, false, t)
            .await
            .is_ok());
    }

    #[serial]
    #[tokio::test]
    async fn checkin_expires_at_uses_club_valid_minutes() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db).await;
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        sqlx::query("UPDATE clubs SET checkin_valid_minutes = 45 WHERE id = $1")
            .bind(&club)
            .execute(&db.pool)
            .await
            .unwrap();
        let t = 1_700_000_123_000_i64;
        let token = db.mint_checkin_qr_token(&club, t).await.unwrap();
        let rec = db.submit_checkin(&a, Some(&dev), &token, t).await.unwrap();
        assert_eq!(rec.created_at, t);
        assert_eq!(rec.expires_at, t + 45 * 60_000);
        let row = sqlx::query("SELECT created_at, expires_at FROM checkins WHERE id = $1")
            .bind(&rec.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        let stored: (i64, i64) = (row.get(0), row.get(1));
        assert_eq!(stored.0 + 45 * 60_000, stored.1);
    }

    #[serial]
    #[tokio::test]
    async fn pilot_session_ttl_is_milliseconds() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, _, _, _) = seed(&db).await;
        let t = now_ms();
        let (_, row) = db.login(&club, "a@test", "password12", t).await.unwrap();
        let exp: i64 =
            sqlx::query_scalar("SELECT expires_at FROM pilot_sessions WHERE pilot_id = $1")
                .bind(&row.id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert!(exp >= t + SESSION_TTL_SEC * 1000);
        assert!(exp < t + SESSION_TTL_SEC * 1000 + 60_000);
    }

    #[serial]
    #[tokio::test]
    async fn start_flight_rejects_foreign_device() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, b, _) = seed(&db).await;
        let dev_a = db.create_device(&a, "a-phone").await.unwrap().0;
        let dev_b = db.create_device(&b, "b-phone").await.unwrap().0;
        let t = now_ms();
        let qr = db.mint_checkin_qr_token(&club, t).await.unwrap();
        db.submit_checkin(&b, Some(&dev_b), &qr, t).await.unwrap();
        assert_eq!(
            db.start_flight(&b, &dev_a, "N123", FlightVisibility::Club, false, t)
                .await,
            Err(StartFlightError::DeviceMismatch)
        );
    }

    #[serial]
    #[tokio::test]
    async fn display_map_omits_private_flights() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a, _, _) = seed(&db).await;
        let admin = a.clone();
        let dev = db.create_device(&a, "phone").await.unwrap().0;
        let t = now_ms();
        let qr = db.mint_checkin_qr_token(&club, t).await.unwrap();
        db.submit_checkin(&a, Some(&dev), &qr, t).await.unwrap();
        let fid = db
            .start_flight(&a, &dev, "N123", FlightVisibility::Private, false, t)
            .await
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
        .await
        .unwrap();
        assert!(db.display_map(&club, &admin).await.is_empty());
    }

    async fn empty_club_with_approved_pilot(db: &ClubDb) -> (String, String) {
        let club = db.default_club_id().await.unwrap();
        let p = db
            .register(&club, "P", "p@test", "L1", "password12")
            .await
            .unwrap();
        db.approve_pilot(&p).await.unwrap();
        (club, p)
    }

    #[serial]
    #[tokio::test]
    async fn access_request_approve_then_start() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, p) = empty_club_with_approved_pilot(&db).await;
        let admin = db
            .register(&club, "A", "admin@test", "", "password12")
            .await
            .unwrap();
        db.approve_pilot(&admin).await.unwrap();
        sqlx::query("UPDATE pilots SET role = 'admin' WHERE id = $1")
            .bind(&admin)
            .execute(&db.pool)
            .await
            .unwrap();
        db.ensure_membership_row(&admin, &club, "admin", "approved")
            .await
            .unwrap();
        let dev = db.create_device(&p, "phone").await.unwrap().0;
        db.issue_open_checkin(&p, &dev, &club, now_ms())
            .await
            .unwrap();
        let rid = db
            .create_access_request(&p, "CC-ABC", None, Some("C152"), now_ms())
            .await
            .unwrap();
        db.approve_access_request(&admin, &rid, None, now_ms())
            .await
            .unwrap();
        let tails = db.me_tails(&p, now_ms()).await.unwrap();
        assert!(tails.iter().any(|t| t.tail == "CC-ABC"));
        db.issue_open_checkin(&p, &dev, &club, now_ms())
            .await
            .unwrap();
        assert!(db
            .start_flight(&p, &dev, "CC-ABC", FlightVisibility::Club, false, now_ms())
            .await
            .is_ok());
    }

    #[serial]
    #[tokio::test]
    async fn access_request_reject_blocks_start() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, p) = empty_club_with_approved_pilot(&db).await;
        let admin = db
            .register(&club, "A", "admin@test", "", "password12")
            .await
            .unwrap();
        db.approve_pilot(&admin).await.unwrap();
        sqlx::query("UPDATE pilots SET role = 'admin' WHERE id = $1")
            .bind(&admin)
            .execute(&db.pool)
            .await
            .unwrap();
        db.ensure_membership_row(&admin, &club, "admin", "approved")
            .await
            .unwrap();
        let dev = db.create_device(&p, "phone").await.unwrap().0;
        db.issue_open_checkin(&p, &dev, &club, now_ms())
            .await
            .unwrap();
        let rid = db
            .create_access_request(&p, "CC-XYZ", None, Some("DA20"), now_ms())
            .await
            .unwrap();
        db.reject_access_request(&admin, &rid, "not yet", now_ms())
            .await
            .unwrap();
        let list = db.list_pilot_access_requests(&p).await;
        assert_eq!(list[0].status, "rejected");
        assert_eq!(list[0].reject_reason.as_deref(), Some("not yet"));
        db.upsert_type(&club, "DA20").await.unwrap();
        db.upsert_aircraft(&club, "CC-XYZ", "DA20", true)
            .await
            .unwrap();
        assert!(matches!(
            db.start_flight(&p, &dev, "CC-XYZ", FlightVisibility::Club, false, now_ms())
                .await,
            Err(StartFlightError::NotCleared { .. })
        ));
    }

    #[serial]
    #[tokio::test]
    async fn pending_access_request_cannot_start_without_checkout() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, p) = empty_club_with_approved_pilot(&db).await;
        let dev = db.create_device(&p, "phone").await.unwrap().0;
        db.issue_open_checkin(&p, &dev, &club, now_ms())
            .await
            .unwrap();
        db.create_access_request(&p, "CC-NOP", None, Some("T1"), now_ms())
            .await
            .unwrap();
        db.upsert_type(&club, "T1").await.unwrap();
        db.upsert_aircraft(&club, "CC-NOP", "T1", true)
            .await
            .unwrap();
        assert!(matches!(
            db.start_flight(&p, &dev, "CC-NOP", FlightVisibility::Club, false, now_ms())
                .await,
            Err(StartFlightError::NotCleared { .. })
        ));
    }

    #[serial]
    #[tokio::test]
    async fn add_checkout_unknown_tail_errors() {
        let db = ClubDb::open_test().await.unwrap();
        let (club, a) = empty_club_with_approved_pilot(&db).await;
        db.upsert_type(&club, "C152").await.unwrap();
        assert!(matches!(
            db.add_checkout(&a, "C152", Some("NOPE"), None).await,
            Err(CheckoutError::UnknownTail)
        ));
    }
}
