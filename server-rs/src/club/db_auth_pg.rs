use crate::club::{
    auth::{self, digest_token, new_id, new_session_id, DUMMY_PASSWORD_HASH, SESSION_TTL_SEC},
    db::{ClubDb, DbError, LoginError, PilotRow},
};
use sqlx::Row;

pub const MAGIC_TTL_MS: i64 = 15 * 60 * 1000;
pub const LOGIN_THROTTLE_WINDOW_MS: i64 = 60 * 1000;
pub const LOGIN_THROTTLE_MAX: i64 = 8;
pub const MAGIC_SEND_WINDOW_MS: i64 = 3600 * 1000;
pub const MAGIC_SEND_MAX_PER_EMAIL: i64 = 5;
pub const MAGIC_SEND_MAX_PER_IP: i64 = 20;
pub const GUEST_SESSION_TTL_MS: i64 = 7 * 24 * 3600 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MagicConsumeError {
    Invalid,
    Expired,
    Used,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestClaimError {
    InvalidSession,
    EmailExists,
    WeakPassword,
    Db,
}

impl ClubDb {
    pub async fn create_pilot_session(&self, pilot_id: &str, now: i64) -> Result<String, DbError> {
        let session_id = new_session_id();
        let exp = now + SESSION_TTL_SEC * 1000;
        sqlx::query("INSERT INTO pilot_sessions (id, pilot_id, expires_at) VALUES ($1, $2, $3)")
            .bind(&session_id)
            .bind(pilot_id)
            .bind(exp)
            .execute(&self.pool)
            .await?;
        Ok(session_id)
    }

    pub async fn login_throttled(&self, ip: &str, now: i64) -> Result<(), LoginError> {
        let cutoff = now - LOGIN_THROTTLE_WINDOW_MS;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM login_attempts WHERE ip = $1 AND ts > $2",
        )
        .bind(ip)
        .bind(cutoff)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| LoginError::Invalid)?;
        if count >= LOGIN_THROTTLE_MAX {
            return Err(LoginError::Throttled);
        }
        Ok(())
    }

    pub async fn record_login_attempt(
        &self,
        ip: &str,
        email: &str,
        now: i64,
    ) -> Result<(), DbError> {
        sqlx::query("INSERT INTO login_attempts (ip, email, ts) VALUES ($1, $2, $3)")
            .bind(ip)
            .bind(email)
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn login_password(
        &self,
        club_id: &str,
        email: &str,
        password: &str,
        now: i64,
        ip: &str,
    ) -> Result<(String, PilotRow), LoginError> {
        self.login_throttled(ip, now).await?;
        let email_norm = email.trim();
        let row = sqlx::query(
            "SELECT id, club_id, name, email, license_no, status, role, locale, password_hash
             FROM pilots WHERE club_id = $1 AND lower(email) = lower($2)",
        )
        .bind(club_id)
        .bind(email_norm)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| LoginError::Invalid)?;

        let verify_hash = |hash: &str| auth::verify_password(password, hash);

        let Some(row) = row else {
            let _ = verify_hash(DUMMY_PASSWORD_HASH);
            let _ = self.record_login_attempt(ip, email_norm, now).await;
            return Err(LoginError::Invalid);
        };

        let pilot = row_to_pilot(&row).map_err(|_| LoginError::Invalid)?;
        let hash: String = row.try_get(8).map_err(|_| LoginError::Invalid)?;
        if hash.is_empty() {
            let _ = verify_hash(DUMMY_PASSWORD_HASH);
            let _ = self.record_login_attempt(ip, email_norm, now).await;
            return Err(LoginError::Invalid);
        }
        if !verify_hash(&hash) {
            let _ = self.record_login_attempt(ip, email_norm, now).await;
            return Err(LoginError::Invalid);
        }
        let session_id = self
            .create_pilot_session(&pilot.id, now)
            .await
            .map_err(|_| LoginError::Invalid)?;
        Ok((session_id, pilot))
    }

    pub async fn magic_send_allowed(&self, email: &str, ip: &str, now: i64) -> bool {
        let email = email.trim().to_ascii_lowercase();
        let cutoff = now - MAGIC_SEND_WINDOW_MS;
        let email_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM magic_send_log WHERE lower(email) = $1 AND ts > $2",
        )
        .bind(&email)
        .bind(cutoff)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        if email_count >= MAGIC_SEND_MAX_PER_EMAIL {
            return false;
        }
        let ip_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM magic_send_log WHERE ip = $1 AND ts > $2",
        )
        .bind(ip)
        .bind(cutoff)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        ip_count < MAGIC_SEND_MAX_PER_IP
    }

    pub async fn log_magic_send(&self, email: &str, ip: &str, now: i64) -> Result<(), DbError> {
        sqlx::query("INSERT INTO magic_send_log (email, ip, ts) VALUES ($1, $2, $3)")
            .bind(email.trim().to_ascii_lowercase())
            .bind(ip)
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn issue_magic_login(
        &self,
        club_id: &str,
        email: &str,
        now: i64,
    ) -> Result<Option<String>, DbError> {
        let email_norm = email.trim();
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM pilots WHERE club_id = $1 AND lower(email) = lower($2) AND status <> 'guest'
             )",
        )
        .bind(club_id)
        .bind(email_norm)
        .fetch_one(&self.pool)
        .await?;
        if !exists {
            return Ok(None);
        }
        let raw_token = auth::new_share_token();
        let token_hash = digest_token(&raw_token);
        let id = new_id();
        let expires = now + MAGIC_TTL_MS;
        sqlx::query(
            "INSERT INTO magic_login_tokens (id, email, token_hash, created_at, expires_at, purpose)
             VALUES ($1, $2, $3, $4, $5, 'login')",
        )
        .bind(&id)
        .bind(email_norm.to_ascii_lowercase())
        .bind(&token_hash)
        .bind(now)
        .bind(expires)
        .execute(&self.pool)
        .await?;
        Ok(Some(raw_token))
    }

    pub async fn consume_magic_login(
        &self,
        raw_token: &str,
        club_id: &str,
        now: i64,
    ) -> Result<(String, PilotRow), MagicConsumeError> {
        let token_hash = digest_token(raw_token.trim());
        let row = sqlx::query(
            "SELECT id, email, expires_at, used_at FROM magic_login_tokens WHERE token_hash = $1",
        )
        .bind(&token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| MagicConsumeError::Invalid)?;
        let Some(row) = row else {
            return Err(MagicConsumeError::Invalid);
        };
        let id: String = row.try_get(0).map_err(|_| MagicConsumeError::Invalid)?;
        let email: String = row.try_get(1).map_err(|_| MagicConsumeError::Invalid)?;
        let expires_at: i64 = row.try_get(2).map_err(|_| MagicConsumeError::Invalid)?;
        let used_at: Option<i64> = row.try_get(3).map_err(|_| MagicConsumeError::Invalid)?;
        if used_at.is_some() {
            return Err(MagicConsumeError::Used);
        }
        if now > expires_at {
            return Err(MagicConsumeError::Expired);
        }
        let updated = sqlx::query(
            "UPDATE magic_login_tokens SET used_at = $1 WHERE id = $2 AND used_at IS NULL",
        )
        .bind(now)
        .bind(&id)
        .execute(&self.pool)
        .await
        .map_err(|_| MagicConsumeError::Invalid)?;
        if updated.rows_affected() == 0 {
            return Err(MagicConsumeError::Used);
        }
        let pilot_row = sqlx::query(
            "SELECT id, club_id, name, email, license_no, status, role, locale
             FROM pilots WHERE club_id = $1 AND lower(email) = lower($2)",
        )
        .bind(club_id)
        .bind(&email)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| MagicConsumeError::Invalid)?;
        let Some(pilot_row) = pilot_row else {
            return Err(MagicConsumeError::Invalid);
        };
        let pilot = row_to_pilot(&pilot_row).map_err(|_| MagicConsumeError::Invalid)?;
        let session_id = self
            .create_pilot_session(&pilot.id, now)
            .await
            .map_err(|_| MagicConsumeError::Invalid)?;
        Ok((session_id, pilot))
    }

    pub async fn create_guest_session(
        &self,
        club_id: &str,
        name: &str,
        now: i64,
    ) -> Result<(String, String, PilotRow), DbError> {
        let guest_pilot_id = new_id();
        let guest_email = format!("guest-{guest_pilot_id}@guest.invalid");
        let display_name = if name.trim().is_empty() {
            "Guest"
        } else {
            name.trim()
        };
        sqlx::query(
            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
             VALUES ($1, $2, $3, $4, '', 'guest', 'pilot', '', $5)",
        )
        .bind(&guest_pilot_id)
        .bind(club_id)
        .bind(display_name)
        .bind(&guest_email)
        .bind(now)
        .execute(&self.pool)
        .await?;
        let session_id = new_id();
        let expires = now + GUEST_SESSION_TTL_MS;
        sqlx::query(
            "INSERT INTO guest_sessions (id, club_id, guest_pilot_id, created_at, expires_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&session_id)
        .bind(club_id)
        .bind(&guest_pilot_id)
        .bind(now)
        .bind(expires)
        .execute(&self.pool)
        .await?;
        let pilot_session = self.create_pilot_session(&guest_pilot_id, now).await?;
        let pilot = PilotRow {
            id: guest_pilot_id.clone(),
            club_id: club_id.to_string(),
            name: display_name.to_string(),
            email: guest_email,
            license_no: String::new(),
            status: "guest".to_string(),
            role: "pilot".to_string(),
            locale: "es".to_string(),
        };
        Ok((pilot_session, session_id, pilot))
    }

    pub async fn guest_session_for_pilot(&self, pilot_id: &str, now: i64) -> Option<String> {
        sqlx::query_scalar(
            "SELECT id FROM guest_sessions
             WHERE guest_pilot_id = $1 AND claimed_at IS NULL AND expires_at > $2",
        )
        .bind(pilot_id)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    pub async fn claim_guest(
        &self,
        guest_session_id: &str,
        name: &str,
        email: &str,
        license_no: &str,
        password: Option<&str>,
        now: i64,
    ) -> Result<PilotRow, GuestClaimError> {
        let row = sqlx::query(
            "SELECT club_id, guest_pilot_id, claimed_at, expires_at
             FROM guest_sessions WHERE id = $1",
        )
        .bind(guest_session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| GuestClaimError::Db)?;
        let Some(row) = row else {
            return Err(GuestClaimError::InvalidSession);
        };
        let club_id: String = row.try_get(0).map_err(|_| GuestClaimError::Db)?;
        let guest_pilot_id: String = row.try_get(1).map_err(|_| GuestClaimError::Db)?;
        let claimed_at: Option<i64> = row.try_get(2).map_err(|_| GuestClaimError::Db)?;
        let expires_at: i64 = row.try_get(3).map_err(|_| GuestClaimError::Db)?;
        if claimed_at.is_some() || now > expires_at {
            return Err(GuestClaimError::InvalidSession);
        }
        let email = email.trim();
        if email.is_empty() {
            return Err(GuestClaimError::InvalidSession);
        }
        let dup: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM pilots
                WHERE club_id = $1 AND lower(email) = lower($2) AND id <> $3
             )",
        )
        .bind(&club_id)
        .bind(email)
        .bind(&guest_pilot_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| GuestClaimError::Db)?;
        if dup {
            return Err(GuestClaimError::EmailExists);
        }
        let password_hash = match password {
            Some(p) if !p.is_empty() => {
                if p.len() < 8 {
                    return Err(GuestClaimError::WeakPassword);
                }
                auth::hash_password(p).map_err(|_| GuestClaimError::Db)?
            }
            _ => String::new(),
        };
        let display_name = if name.trim().is_empty() {
            "Pilot"
        } else {
            name.trim()
        };
        sqlx::query(
            "UPDATE pilots SET name = $1, email = $2, license_no = $3, password_hash = $4,
             status = 'pending' WHERE id = $5",
        )
        .bind(display_name)
        .bind(email)
        .bind(license_no.trim())
        .bind(&password_hash)
        .bind(&guest_pilot_id)
        .execute(&self.pool)
        .await
        .map_err(|_| GuestClaimError::Db)?;
        sqlx::query("UPDATE guest_sessions SET claimed_at = $1 WHERE id = $2")
            .bind(now)
            .bind(guest_session_id)
            .execute(&self.pool)
            .await
            .map_err(|_| GuestClaimError::Db)?;
        let pilot_row = sqlx::query(
            "SELECT id, club_id, name, email, license_no, status, role, locale
             FROM pilots WHERE id = $1",
        )
        .bind(&guest_pilot_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| GuestClaimError::Db)?;
        row_to_pilot(&pilot_row).map_err(|_| GuestClaimError::Db)
    }

    pub async fn revoke_all_devices(&self, pilot_id: &str) -> Result<(), DbError> {
        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM devices WHERE pilot_id = $1")
            .bind(pilot_id)
            .fetch_all(&self.pool)
            .await?;
        for device_id in ids {
            let _ = self.revoke_device_by_id(pilot_id, &device_id).await;
        }
        Ok(())
    }
}

fn row_to_pilot(row: &sqlx::postgres::PgRow) -> Result<PilotRow, sqlx::Error> {
    Ok(PilotRow {
        id: row.try_get(0)?,
        club_id: row.try_get(1)?,
        name: row.try_get(2)?,
        email: row.try_get(3)?,
        license_no: row.try_get(4)?,
        status: row.try_get(5)?,
        role: row.try_get(6)?,
        locale: row.try_get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::club::auth::hash_password;
    use crate::club::db::now_ms;
    use serial_test::serial;

    fn token_len_ok() -> bool {
        true
    }

    #[serial]
    #[tokio::test]
    async fn magic_consume_single_use_and_session() {
        let _ = token_len_ok();
        let db = ClubDb::open_test().await.unwrap();
        let club = db.default_club_id().await.unwrap();
        let hash = hash_password("password12").unwrap();
        let pid = new_id();
        sqlx::query(
            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
             VALUES ($1, $2, 'M', 'magic@test', '', 'approved', 'pilot', $3, $4)",
        )
        .bind(&pid)
        .bind(&club)
        .bind(&hash)
        .bind(now_ms())
        .execute(&db.pool)
        .await
        .unwrap();
        let now = now_ms();
        let raw = db
            .issue_magic_login(&club, "magic@test", now)
            .await
            .unwrap()
            .unwrap();
        let (sess, pilot) = db.consume_magic_login(&raw, &club, now).await.unwrap();
        assert_eq!(pilot.email, "magic@test");
        assert!(!sess.is_empty());
        assert!(matches!(
            db.consume_magic_login(&raw, &club, now).await,
            Err(MagicConsumeError::Used)
        ));
    }

    #[serial]
    #[tokio::test]
    async fn login_throttle_blocks_after_max() {
        let db = ClubDb::open_test().await.unwrap();
        let club = db.default_club_id().await.unwrap();
        let now = now_ms();
        let ip = "203.0.113.50";
        for _ in 0..LOGIN_THROTTLE_MAX {
            let _ = db.record_login_attempt(ip, "x@test", now).await;
        }
        assert!(db.login_throttled(ip, now).await.is_err());
        let _ = db
            .login_password(&club, "nobody@test", "wrong", now, ip)
            .await;
    }

    #[serial]
    #[tokio::test]
    async fn guest_claim_migrates_pilot() {
        let db = ClubDb::open_test().await.unwrap();
        let club = db.default_club_id().await.unwrap();
        let now = now_ms();
        let (_pilot_sess, guest_id, guest) =
            db.create_guest_session(&club, "Try", now).await.unwrap();
        assert_eq!(guest.status, "guest");
        let claimed = db
            .claim_guest(
                &guest_id,
                "Full Name",
                "claimed@test",
                "L-9",
                Some("password12"),
                now,
            )
            .await
            .unwrap();
        assert_eq!(claimed.email, "claimed@test");
        assert_eq!(claimed.status, "pending");
    }

    #[serial]
    #[tokio::test]
    async fn password_login_dummy_hash_when_missing_user() {
        let db = ClubDb::open_test().await.unwrap();
        let club = db.default_club_id().await.unwrap();
        let now = now_ms();
        let ip = "198.51.100.1";
        assert!(db
            .login_password(&club, "missing@test", "whatever", now, ip)
            .await
            .is_err());
    }
}
