//! Global accounts, account sessions, club join, and migration test hooks.

use crate::club::{
    auth::{self, digest_token, new_id, new_session_id, SESSION_TTL_SEC},
    db::{now_ms, ClubDb, DbError, LoginError, PilotRow, RegisterError},
    membership_status::{membership_role_is_admin, MembershipStatus},
};
use sqlx::Row;

pub const ACCOUNT_MAGIC_TTL_MS: i64 = 15 * 60 * 1000;

#[derive(Debug, Clone)]
pub struct AccountRow {
    pub id: String,
    pub email: String,
    pub email_verified_at: Option<i64>,
    #[allow(dead_code)]
    pub global_role: String,
    pub suspended_at: Option<i64>,
    pub locale: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MembershipRow {
    pub id: String,
    #[allow(dead_code)]
    pub pilot_id: String,
    pub club_id: String,
    pub club_slug: Option<String>,
    pub club_name: Option<String>,
    pub role: String,
    pub status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountSignupError {
    WeakPassword,
    TooLong,
    WrongPassword,
    AuthRequired,
    #[allow(dead_code)]
    EmailExists,
    Db,
}

pub const ACCOUNT_PASSWORD_RECENT_AUTH_MS: i64 = 15 * 60 * 1000;

pub const ACCOUNT_SESSION_AUTH_MAGIC: &str = "magic";
pub const ACCOUNT_SESSION_AUTH_PASSWORD: &str = "password";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountJoinError {
    EmailExists,
    #[allow(dead_code)]
    WeakPassword,
    ClubClosed,
    ClubSuspended,
    AccountSuspended,
    Db,
}

impl ClubDb {
    pub async fn account_id_for_email(&self, email: &str) -> Option<String> {
        self.account_by_email(email).await.map(|a| a.id)
    }

    pub async fn account_by_email(&self, email: &str) -> Option<AccountRow> {
        let row = sqlx::query(
            "SELECT id, email, email_verified_at, global_role, suspended_at, locale
             FROM accounts WHERE lower(email) = lower($1)",
        )
        .bind(email.trim())
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        Some(row_to_account(&row))
    }

    pub async fn account_by_id(&self, account_id: &str) -> Option<AccountRow> {
        let row = sqlx::query(
            "SELECT id, email, email_verified_at, global_role, suspended_at, locale
             FROM accounts WHERE id = $1",
        )
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        Some(row_to_account(&row))
    }

    /// Link pilots created without accounts (e.g. demo seed after migrations) to `accounts` +
    /// `club_memberships`, copying password hashes for account login in e2e/CI.
    pub async fn backfill_accounts_from_pilots(&self) -> Result<(), DbError> {
        let now = now_ms();
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query(
            "SELECT id, club_id, email, password_hash, role, status, created_at, email_verified_at
             FROM pilots
             WHERE account_id IS NULL
               AND trim(email) <> ''
               AND email NOT LIKE 'guest-%@guest.invalid'
               AND (status <> 'guest' OR email NOT LIKE 'guest-%@guest.invalid')",
        )
        .fetch_all(&mut *tx)
        .await?;
        for row in rows {
            let pilot_id: String = row.try_get("id")?;
            let club_id: String = row.try_get("club_id")?;
            let email: String = row.try_get("email")?;
            let password_hash: String = row.try_get("password_hash").unwrap_or_default();
            let role: String = row.try_get("role")?;
            let status: String = row.try_get("status")?;
            let pilot_created: i64 = row.try_get("created_at").unwrap_or(now);
            let pilot_verified: Option<i64> = row.try_get("email_verified_at").ok();
            let email = email.trim().to_ascii_lowercase();
            if status == "guest" {
                continue;
            }
            let account_id: String = if let Some(acct) =
                sqlx::query_scalar("SELECT id FROM accounts WHERE lower(email) = lower($1)")
                    .bind(&email)
                    .fetch_optional(&mut *tx)
                    .await?
            {
                acct
            } else {
                let id = new_id();
                let verified_at: Option<i64> = sqlx::query_scalar(
                    "SELECT max(email_verified_at) FROM pilots WHERE lower(trim(email)) = $1",
                )
                .bind(&email)
                .fetch_optional(&mut *tx)
                .await?
                .flatten();
                sqlx::query(
                    "INSERT INTO accounts (id, email, password_hash, created_at, email_verified_at)
                     VALUES ($1, $2, NULL, $3, $4)",
                )
                .bind(&id)
                .bind(&email)
                .bind(pilot_created)
                .bind(verified_at)
                .execute(&mut *tx)
                .await?;
                id
            };
            let squat_password = pilot_verified.is_none() && !password_hash.trim().is_empty();
            if status == "approved"
                && pilot_verified.is_some()
                && !password_hash.trim().is_empty()
                && !squat_password
            {
                let _ = sqlx::query(
                    "UPDATE accounts SET password_hash = coalesce(nullif(trim(password_hash), ''), $1)
                     WHERE id = $2 AND email_verified_at IS NOT NULL
                       AND coalesce(trim(password_hash), '') = ''",
                )
                .bind(password_hash.trim())
                .bind(&account_id)
                .execute(&mut *tx)
                .await;
            }
            sqlx::query("UPDATE pilots SET account_id = $1 WHERE id = $2")
                .bind(&account_id)
                .bind(&pilot_id)
                .execute(&mut *tx)
                .await?;
            let _ = squat_password;
            let membership_role = if membership_role_is_admin(&role) {
                "admin"
            } else if role == "staff" {
                "staff"
            } else {
                "pilot"
            };
            let membership_status = match status.as_str() {
                "approved" => "active",
                "suspended" => "suspended",
                _ => "pending",
            };
            let member_id = new_id();
            sqlx::query(
                "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (pilot_id, club_id) DO UPDATE SET
                   status = CASE
                     WHEN club_memberships.status IN ('active', 'suspended') THEN club_memberships.status
                     ELSE EXCLUDED.status
                   END,
                   role = club_memberships.role",
            )
            .bind(&member_id)
            .bind(&pilot_id)
            .bind(&club_id)
            .bind(membership_role)
            .bind(membership_status)
            .bind(pilot_created)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Create or return existing account by email (random id on create).
    pub async fn ensure_account_for_email(
        &self,
        email: &str,
        password: Option<&str>,
        now: i64,
    ) -> Result<String, AccountSignupError> {
        let email = email.trim().to_ascii_lowercase();
        if !email.contains('@') {
            return Err(AccountSignupError::Db);
        }
        if let Some(p) = password.filter(|s| !s.is_empty()) {
            if p.len() < 8 {
                return Err(AccountSignupError::WeakPassword);
            }
        }
        if let Some(existing) = self.account_by_email(&email).await {
            return Ok(existing.id);
        }
        let id = new_id();
        let ins = sqlx::query(
            "INSERT INTO accounts (id, email, password_hash, created_at)
             VALUES ($1, $2, NULL, $3)",
        )
        .bind(&id)
        .bind(&email)
        .bind(now)
        .execute(&self.pool)
        .await;
        match ins {
            Ok(_) => Ok(id),
            Err(e) => {
                if let Some(db_err) = e.as_database_error() {
                    if db_err.code() == Some(std::borrow::Cow::Borrowed("23505")) {
                        return self
                            .account_id_for_email(&email)
                            .await
                            .ok_or(AccountSignupError::Db);
                    }
                }
                Err(AccountSignupError::Db)
            }
        }
    }

    #[allow(dead_code)]
    pub async fn create_account(
        &self,
        email: &str,
        _password: Option<&str>,
        now: i64,
    ) -> Result<String, AccountSignupError> {
        self.ensure_account_for_email(email, None, now).await
    }

    #[allow(dead_code)]
    pub async fn issue_account_magic_signup(
        &self,
        email: &str,
        password: Option<&str>,
        signup_nonce: Option<&str>,
        now: i64,
    ) -> Result<String, DbError> {
        self.issue_account_magic_signup_with_club(email, password, signup_nonce, None, now)
            .await
    }

    pub async fn issue_account_magic_signup_with_club(
        &self,
        email: &str,
        password: Option<&str>,
        signup_nonce: Option<&str>,
        club_id: Option<&str>,
        now: i64,
    ) -> Result<String, DbError> {
        let pending_password_hash: Option<String> = match password.filter(|p| !p.is_empty()) {
            Some(p) => Some(auth::hash_password(p).map_err(|_| DbError::protocol("hash"))?),
            None => None,
        };
        let signup_nonce_hash = signup_nonce.filter(|s| !s.is_empty()).map(digest_token);
        let club_id = club_id.map(str::trim).filter(|s| !s.is_empty());
        let email_lc = email.trim().to_ascii_lowercase();
        sqlx::query(
            "UPDATE magic_login_tokens SET used_at = $1
             WHERE lower(email) = $2 AND purpose = 'account_signup' AND used_at IS NULL",
        )
        .bind(now)
        .bind(&email_lc)
        .execute(&self.pool)
        .await?;
        let raw = auth::new_share_token();
        let token_hash = digest_token(&raw);
        let id = new_id();
        let expires = now + ACCOUNT_MAGIC_TTL_MS;
        let account_id: Option<String> = self.account_by_email(email).await.map(|a| a.id);
        sqlx::query(
            "INSERT INTO magic_login_tokens (id, email, token_hash, created_at, expires_at, purpose, account_id, pending_password_hash, signup_nonce_hash, club_id)
             VALUES ($1, $2, $3, $4, $5, 'account_signup', $6, $7, $8, $9)",
        )
        .bind(&id)
        .bind(&email_lc)
        .bind(&token_hash)
        .bind(now)
        .bind(expires)
        .bind(account_id.as_deref())
        .bind(pending_password_hash.as_deref())
        .bind(signup_nonce_hash.as_deref())
        .bind(club_id)
        .execute(&self.pool)
        .await?;
        Ok(raw)
    }

    pub async fn issue_account_magic_login(
        &self,
        email: &str,
        now: i64,
    ) -> Result<Option<String>, DbError> {
        let Some(acct) = self.account_by_email(email).await else {
            return Ok(None);
        };
        if acct.suspended_at.is_some() {
            return Ok(None);
        }
        let raw = auth::new_share_token();
        let token_hash = digest_token(&raw);
        let id = new_id();
        let expires = now + ACCOUNT_MAGIC_TTL_MS;
        sqlx::query(
            "INSERT INTO magic_login_tokens (id, email, token_hash, created_at, expires_at, purpose, account_id)
             VALUES ($1, $2, $3, $4, $5, 'account_login', $6)",
        )
        .bind(&id)
        .bind(email.trim().to_ascii_lowercase())
        .bind(&token_hash)
        .bind(now)
        .bind(expires)
        .bind(&acct.id)
        .execute(&self.pool)
        .await?;
        Ok(Some(raw))
    }

    pub async fn peek_magic_token_purpose(&self, raw_token: &str) -> Option<String> {
        let token_hash = digest_token(raw_token.trim());
        sqlx::query_scalar("SELECT purpose FROM magic_login_tokens WHERE token_hash = $1")
            .bind(&token_hash)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    fn signup_nonce_matches(token_nonce: Option<&str>, cookie_nonce: Option<&str>) -> bool {
        match (token_nonce, cookie_nonce.filter(|s| !s.is_empty())) {
            (Some(tn), Some(cn)) => {
                let a = tn.as_bytes();
                let b = digest_token(cn);
                a.len() == b.len() && subtle::ConstantTimeEq::ct_eq(a, b.as_bytes()).into()
            }
            _ => false,
        }
    }

    pub async fn consume_account_magic(
        &self,
        raw_token: &str,
        signup_nonce: Option<&str>,
        now: i64,
    ) -> Result<AccountRow, crate::club::db_auth_pg::MagicConsumeError> {
        use crate::club::db_auth_pg::MagicConsumeError;
        let token_hash = digest_token(raw_token.trim());
        let row = sqlx::query(
            "SELECT id, email, expires_at, used_at, purpose, account_id, pending_password_hash, signup_nonce_hash, club_id
             FROM magic_login_tokens WHERE token_hash = $1",
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
        let purpose: String = row.try_get(4).unwrap_or_default();
        let account_id: Option<String> = row.try_get(5).ok();
        let pending_password_hash: Option<String> = row.try_get(6).ok().flatten();
        let token_signup_nonce_hash: Option<String> = row.try_get(7).ok().flatten();
        let token_club_id: Option<String> = row.try_get(8).ok().flatten();
        if purpose != "account_signup" && purpose != "account_login" {
            return Err(MagicConsumeError::Invalid);
        }
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
        let (acct_id, newly_created) = if let Some(aid) = account_id {
            (aid, false)
        } else if let Some(row) = self.account_by_email(&email).await {
            (row.id, false)
        } else {
            let aid = new_id();
            sqlx::query(
                "INSERT INTO accounts (id, email, created_at, email_verified_at)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(&aid)
            .bind(email.trim().to_ascii_lowercase())
            .bind(now)
            .bind(if purpose.as_str() == "account_signup" {
                Some(now)
            } else {
                None
            })
            .execute(&self.pool)
            .await
            .map_err(|_| MagicConsumeError::Invalid)?;
            (aid, true)
        };
        let was_unverified: bool =
            sqlx::query_scalar("SELECT email_verified_at IS NULL FROM accounts WHERE id = $1")
                .bind(&acct_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|_| MagicConsumeError::Invalid)?
                .unwrap_or(false);
        if purpose == "account_signup" || purpose == "account_login" {
            if was_unverified {
                sqlx::query("UPDATE accounts SET password_hash = NULL WHERE id = $1")
                    .bind(&acct_id)
                    .execute(&self.pool)
                    .await
                    .map_err(|_| MagicConsumeError::Invalid)?;
                sqlx::query("DELETE FROM account_sessions WHERE account_id = $1")
                    .bind(&acct_id)
                    .execute(&self.pool)
                    .await
                    .map_err(|_| MagicConsumeError::Invalid)?;
            }
            sqlx::query(
                "UPDATE accounts SET email_verified_at = coalesce(email_verified_at, $1) WHERE id = $2",
            )
            .bind(now)
            .bind(&acct_id)
            .execute(&self.pool)
            .await
            .map_err(|_| MagicConsumeError::Invalid)?;
            if was_unverified {
                let apply_pending = pending_password_hash
                    .as_deref()
                    .filter(|h| !h.is_empty())
                    .filter(|_| {
                        purpose == "account_signup"
                            && Self::signup_nonce_matches(
                                token_signup_nonce_hash.as_deref(),
                                signup_nonce,
                            )
                    });
                if let Some(hash) = apply_pending {
                    sqlx::query("UPDATE accounts SET password_hash = $1 WHERE id = $2")
                        .bind(hash)
                        .bind(&acct_id)
                        .execute(&self.pool)
                        .await
                        .map_err(|_| MagicConsumeError::Invalid)?;
                }
            }
        }
        let acct = self
            .account_by_id(&acct_id)
            .await
            .ok_or(MagicConsumeError::Invalid)?;
        if acct.suspended_at.is_some() {
            return Err(MagicConsumeError::Invalid);
        }
        if purpose == "account_signup" {
            if let Some(club_id) = token_club_id.as_deref().filter(|s| !s.is_empty()) {
                let allow_club_join = newly_created
                    || was_unverified
                    || token_signup_nonce_hash.is_none()
                    || Self::signup_nonce_matches(token_signup_nonce_hash.as_deref(), signup_nonce);
                if allow_club_join {
                    let _ = self
                        .join_club_with_account(&acct.id, club_id, "", "", now)
                        .await;
                }
            }
        }
        Ok(acct)
    }

    #[allow(dead_code)]
    pub async fn create_account_session(
        &self,
        account_id: &str,
        active_membership_id: Option<&str>,
        now: i64,
    ) -> Result<String, DbError> {
        self.create_account_session_auth(
            account_id,
            active_membership_id,
            now,
            ACCOUNT_SESSION_AUTH_PASSWORD,
        )
        .await
    }

    pub async fn create_account_session_auth(
        &self,
        account_id: &str,
        active_membership_id: Option<&str>,
        now: i64,
        auth_method: &str,
    ) -> Result<String, DbError> {
        let session_id = new_session_id();
        let session_db_id = digest_token(&session_id);
        let exp = now + SESSION_TTL_SEC * 1000;
        sqlx::query(
            "INSERT INTO account_sessions (id, account_id, active_membership_id, expires_at, created_at, auth_method)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&session_db_id)
        .bind(account_id)
        .bind(active_membership_id)
        .bind(exp)
        .bind(now)
        .bind(auth_method)
        .execute(&self.pool)
        .await?;
        Ok(session_id)
    }

    fn validate_account_password_new(password: &str) -> Result<(), AccountSignupError> {
        if password.len() < 8 {
            return Err(AccountSignupError::WeakPassword);
        }
        if password.len() > auth::ACCOUNT_PASSWORD_MAX_CHARS {
            return Err(AccountSignupError::TooLong);
        }
        #[allow(clippy::needless_as_bytes)]
        if password.as_bytes().len() > auth::ACCOUNT_PASSWORD_MAX_BYTES {
            return Err(AccountSignupError::TooLong);
        }
        Ok(())
    }

    pub async fn account_session_is_fresh(&self, session_raw: &str, now: i64) -> bool {
        let session_db_id = auth::digest_token(session_raw);
        let created: Option<i64> =
            sqlx::query_scalar("SELECT created_at FROM account_sessions WHERE id = $1")
                .bind(&session_db_id)
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten();
        created.is_some_and(|c| c > 0 && now.saturating_sub(c) <= ACCOUNT_PASSWORD_RECENT_AUTH_MS)
    }

    pub async fn account_session_magic_fresh(&self, session_raw: &str, now: i64) -> bool {
        if !self.account_session_is_fresh(session_raw, now).await {
            return false;
        }
        let session_db_id = auth::digest_token(session_raw);
        let method: Option<String> =
            sqlx::query_scalar("SELECT auth_method FROM account_sessions WHERE id = $1")
                .bind(&session_db_id)
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten();
        method.as_deref() == Some(ACCOUNT_SESSION_AUTH_MAGIC)
    }

    pub async fn set_account_password(
        &self,
        account_id: &str,
        new_password: &str,
        now: i64,
        current_password: Option<&str>,
        keep_session_raw: Option<&str>,
    ) -> Result<(), AccountSignupError> {
        Self::validate_account_password_new(new_password)?;
        let verified: Option<i64> =
            sqlx::query_scalar("SELECT email_verified_at FROM accounts WHERE id = $1")
                .bind(account_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|_| AccountSignupError::Db)?
                .flatten();
        if verified.is_none() {
            return Err(AccountSignupError::Db);
        }
        let existing_hash: Option<String> =
            sqlx::query_scalar("SELECT password_hash FROM accounts WHERE id = $1")
                .bind(account_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|_| AccountSignupError::Db)?
                .flatten();
        let session_fresh = if let Some(raw) = keep_session_raw {
            self.account_session_is_fresh(raw, now).await
        } else {
            false
        };
        let magic_fresh = if let Some(raw) = keep_session_raw {
            self.account_session_magic_fresh(raw, now).await
        } else {
            false
        };
        let has_password = existing_hash.as_deref().is_some_and(|h| !h.is_empty());
        let current_ok = existing_hash
            .as_deref()
            .is_some_and(|hash| current_password.is_some_and(|p| auth::verify_password(p, hash)));
        if has_password {
            if current_password.is_some_and(|p| !p.is_empty()) && !current_ok {
                return Err(AccountSignupError::WrongPassword);
            }
            if !current_ok && !magic_fresh {
                return if current_password.is_some() {
                    Err(AccountSignupError::WrongPassword)
                } else {
                    Err(AccountSignupError::AuthRequired)
                };
            }
        } else if !session_fresh {
            return Err(AccountSignupError::AuthRequired);
        }
        let hash = auth::hash_password(new_password).map_err(|_| AccountSignupError::Db)?;
        let keep_id = keep_session_raw.map(auth::digest_token);
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AccountSignupError::Db)?;
        sqlx::query("UPDATE accounts SET password_hash = $1 WHERE id = $2")
            .bind(&hash)
            .bind(account_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| AccountSignupError::Db)?;
        if let Some(keep) = keep_id.as_deref() {
            sqlx::query(
                "UPDATE account_sessions SET auth_method = $1 WHERE id = $2 AND account_id = $3",
            )
            .bind(ACCOUNT_SESSION_AUTH_PASSWORD)
            .bind(keep)
            .bind(account_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| AccountSignupError::Db)?;
            sqlx::query("DELETE FROM account_sessions WHERE account_id = $1 AND id <> $2")
                .bind(account_id)
                .bind(keep)
                .execute(&mut *tx)
                .await
                .map_err(|_| AccountSignupError::Db)?;
        } else {
            sqlx::query("DELETE FROM account_sessions WHERE account_id = $1")
                .bind(account_id)
                .execute(&mut *tx)
                .await
                .map_err(|_| AccountSignupError::Db)?;
        }
        sqlx::query(
            "DELETE FROM pilot_sessions WHERE pilot_id IN (SELECT id FROM pilots WHERE account_id = $1)",
        )
        .bind(account_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| AccountSignupError::Db)?;
        tx.commit().await.map_err(|_| AccountSignupError::Db)?;
        Ok(())
    }

    pub async fn pilot_for_active_membership(&self, membership_id: &str) -> Option<PilotRow> {
        let row = sqlx::query(
            "SELECT m.pilot_id, m.status, m.club_id FROM club_memberships m WHERE m.id = $1",
        )
        .bind(membership_id)
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        let pilot_id: String = row.try_get(0).ok()?;
        let mem_status: String = row.try_get(1).ok()?;
        if !MembershipStatus::parse(&mem_status).allows_club_api() {
            return None;
        }
        let row = sqlx::query(
            "SELECT id, club_id, name, email, license_no, status, role, locale
             FROM pilots WHERE id = $1",
        )
        .bind(&pilot_id)
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
            locale: row.try_get(7).ok().flatten(),
        })
    }

    pub async fn account_login_password(
        &self,
        email: &str,
        password: &str,
        now: i64,
        ip: &str,
    ) -> Result<(String, AccountRow), LoginError> {
        self.login_throttled(ip, now).await?;
        self.login_throttled_email(email, now).await?;
        let Some(acct) = self.account_by_email(email).await else {
            let _ = auth::verify_password(password, auth::DUMMY_PASSWORD_HASH);
            let _ = self.record_login_attempt(ip, email, now).await;
            return Err(LoginError::Invalid);
        };
        if acct.suspended_at.is_some() {
            return Err(LoginError::Invalid);
        }
        if acct.email_verified_at.is_none() {
            let _ = auth::verify_password(password, auth::DUMMY_PASSWORD_HASH);
            let _ = self.record_login_attempt(ip, email, now).await;
            return Err(LoginError::Invalid);
        }
        let hash: Option<String> =
            sqlx::query_scalar("SELECT password_hash FROM accounts WHERE id = $1")
                .bind(&acct.id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|_| LoginError::Invalid)?
                .flatten();
        let Some(hash) = hash.filter(|h| !h.is_empty()) else {
            let _ = auth::verify_password(password, auth::DUMMY_PASSWORD_HASH);
            let _ = self.record_login_attempt(ip, email, now).await;
            return Err(LoginError::Invalid);
        };
        if !auth::verify_password(password, &hash) {
            let _ = self.record_login_attempt(ip, email, now).await;
            return Err(LoginError::Invalid);
        }
        let session_id = self
            .create_account_session_auth(&acct.id, None, now, ACCOUNT_SESSION_AUTH_PASSWORD)
            .await
            .map_err(|_| LoginError::Invalid)?;
        Ok((session_id, acct))
    }

    pub async fn logout_account_session(&self, session_id: &str) -> Result<(), DbError> {
        let session_db_id = digest_token(session_id);
        sqlx::query("DELETE FROM account_sessions WHERE id = $1")
            .bind(&session_db_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn account_from_session(
        &self,
        session_id: &str,
        now: i64,
    ) -> Option<(AccountRow, Option<String>)> {
        let session_db_id = digest_token(session_id);
        let row = sqlx::query(
            "SELECT a.id, a.email, a.email_verified_at, a.global_role, a.suspended_at, a.locale, s.active_membership_id
             FROM account_sessions s
             JOIN accounts a ON a.id = s.account_id
             WHERE s.id = $1 AND s.expires_at > $2",
        )
        .bind(&session_db_id)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        let acct = AccountRow {
            id: row.try_get(0).ok()?,
            email: row.try_get(1).ok()?,
            email_verified_at: row.try_get(2).ok()?,
            global_role: row.try_get(3).ok()?,
            suspended_at: row.try_get(4).ok()?,
            locale: row.try_get(5).ok().flatten(),
        };
        if acct.suspended_at.is_some() {
            return None;
        }
        let mem: Option<String> = row.try_get(6).ok()?;
        Some((acct, mem))
    }

    pub async fn set_active_membership(
        &self,
        account_session_id: &str,
        membership_id: &str,
        now: i64,
    ) -> Result<PilotRow, DbError> {
        let session_db_id = digest_token(account_session_id);
        let acct = self
            .account_from_session(account_session_id, now)
            .await
            .ok_or_else(|| DbError::protocol("no session"))?;
        let (account, _) = acct;
        if account.email_verified_at.is_none() {
            return Err(DbError::protocol("email not verified"));
        }
        let row = sqlx::query(
            "SELECT m.id, m.pilot_id, m.club_id, m.role, m.status
             FROM club_memberships m
             JOIN pilots p ON p.id = m.pilot_id
             WHERE m.id = $1 AND p.account_id = $2",
        )
        .bind(membership_id)
        .bind(&account.id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Err(DbError::protocol("membership not found"));
        };
        let status: String = row.try_get(4)?;
        if !MembershipStatus::parse(&status).allows_club_api() {
            return Err(DbError::protocol("membership not active"));
        }
        let club_id: String = row.try_get(2)?;
        if self.club_operational(&club_id).await.is_err() {
            return Err(DbError::protocol("club unavailable"));
        }
        sqlx::query(
            "UPDATE account_sessions SET active_membership_id = $1, compat_pilot_session = NULL WHERE id = $2",
        )
        .bind(membership_id)
        .bind(&session_db_id)
        .execute(&self.pool)
        .await?;
        let pilot_id: String = row.try_get(1)?;
        let pilot = sqlx::query(
            "SELECT id, club_id, name, email, license_no, status, role, locale FROM pilots WHERE id = $1",
        )
        .bind(&pilot_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(PilotRow {
            id: pilot.try_get(0)?,
            club_id: pilot.try_get(1)?,
            name: pilot.try_get(2)?,
            email: pilot.try_get(3)?,
            license_no: pilot.try_get(4)?,
            status: pilot.try_get(5)?,
            role: pilot.try_get(6)?,
            locale: pilot.try_get(7)?,
        })
    }

    pub async fn list_account_memberships(&self, account_id: &str) -> Vec<MembershipRow> {
        let rows = sqlx::query(
            "SELECT m.id, m.pilot_id, m.club_id, c.slug, c.name, m.role, m.status
             FROM club_memberships m
             JOIN pilots p ON p.id = m.pilot_id
             JOIN clubs c ON c.id = m.club_id
             WHERE p.account_id = $1
             ORDER BY lower(c.name)",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(MembershipRow {
                    id: r.try_get(0).ok()?,
                    pilot_id: r.try_get(1).ok()?,
                    club_id: r.try_get(2).ok()?,
                    club_slug: r.try_get(3).ok(),
                    club_name: r.try_get(4).ok()?,
                    role: r.try_get(5).ok()?,
                    status: r.try_get(6).ok()?,
                })
            })
            .collect()
    }

    pub async fn join_club_with_account(
        &self,
        account_id: &str,
        club_id: &str,
        name: &str,
        license_no: &str,
        now: i64,
    ) -> Result<String, AccountJoinError> {
        let acct = self
            .account_by_id(account_id)
            .await
            .ok_or(AccountJoinError::Db)?;
        if acct.suspended_at.is_some() {
            return Err(AccountJoinError::AccountSuspended);
        }
        if acct.email_verified_at.is_none() {
            return Err(AccountJoinError::Db);
        }
        if let Some(row) = self.club_public_row(club_id).await {
            if row.suspended_at.is_some() {
                return Err(AccountJoinError::ClubSuspended);
            }
            if !ClubDb::is_public_verified(&row) {
                return Err(AccountJoinError::ClubClosed);
            }
        } else {
            return Err(AccountJoinError::ClubClosed);
        }
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT m.id FROM club_memberships m
             JOIN pilots p ON p.id = m.pilot_id
             WHERE (p.account_id = $1 OR lower(p.email) = lower($3)) AND m.club_id = $2",
        )
        .bind(account_id)
        .bind(club_id)
        .bind(&acct.email)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AccountJoinError::Db)?;
        if existing.is_some() {
            return Err(AccountJoinError::EmailExists);
        }
        let existing_pilot: Option<String> = sqlx::query_scalar(
            "SELECT id FROM pilots WHERE club_id = $1 AND lower(email) = lower($2)",
        )
        .bind(club_id)
        .bind(&acct.email)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AccountJoinError::Db)?;
        if existing_pilot.is_some() {
            return Err(AccountJoinError::EmailExists);
        }
        let pilot_id = new_id();
        let member_id = new_id();
        let display = if name.trim().is_empty() {
            acct.email.split('@').next().unwrap_or("Pilot")
        } else {
            name.trim()
        };
        let pilot_verified = acct.email_verified_at.unwrap_or(now);
        let mut tx = self.pool.begin().await.map_err(|_| AccountJoinError::Db)?;
        let ins_pilot = sqlx::query(
            "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, 'pending', 'pilot', '', $7, $8)",
        )
        .bind(&pilot_id)
        .bind(club_id)
        .bind(account_id)
        .bind(display)
        .bind(&acct.email)
        .bind(license_no.trim())
        .bind(pilot_verified)
        .bind(now)
        .execute(&mut *tx)
        .await;
        if let Err(e) = ins_pilot {
            let _ = tx.rollback().await;
            if let Some(db_err) = e.as_database_error() {
                if db_err.code() == Some(std::borrow::Cow::Borrowed("23505")) {
                    return Err(AccountJoinError::EmailExists);
                }
            }
            return Err(AccountJoinError::Db);
        }
        sqlx::query(
            "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
             VALUES ($1, $2, $3, 'pilot', 'pending', $4)",
        )
        .bind(&member_id)
        .bind(&pilot_id)
        .bind(club_id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|_| AccountJoinError::Db)?;
        tx.commit().await.map_err(|_| AccountJoinError::Db)?;
        Ok(member_id)
    }

    pub async fn register_into_account(
        &self,
        club_id: &str,
        name: &str,
        email: &str,
        license_no: &str,
        password: &str,
        now: i64,
    ) -> Result<(String, String), RegisterError> {
        if password.len() < 8 {
            return Err(RegisterError::WeakPassword);
        }
        let email = email.trim().to_ascii_lowercase();
        let existing_acct = self.account_by_email(&email).await;
        if existing_acct.is_none() {
            self.ensure_account_for_email(&email, None, now)
                .await
                .map_err(|e| match e {
                    AccountSignupError::WeakPassword | AccountSignupError::TooLong => {
                        RegisterError::WeakPassword
                    }
                    AccountSignupError::Db | AccountSignupError::EmailExists => {
                        RegisterError::Db(DbError::protocol("account"))
                    }
                    AccountSignupError::WrongPassword | AccountSignupError::AuthRequired => {
                        RegisterError::Db(DbError::protocol("account"))
                    }
                })?;
        }
        let (pilot_account_id, pilot_password_hash) = match existing_acct.as_ref() {
            Some(a) if a.email_verified_at.is_some() => (Some(a.id.clone()), String::new()),
            Some(_) => (None, String::new()),
            None => (
                None,
                auth::hash_password(password).map_err(|_| RegisterError::Hash)?,
            ),
        };
        let existing_pilot: Option<String> = sqlx::query_scalar(
            "SELECT id FROM pilots WHERE club_id = $1 AND lower(email) = lower($2)",
        )
        .bind(club_id)
        .bind(&email)
        .fetch_optional(&self.pool)
        .await
        .map_err(RegisterError::Db)?;
        if existing_pilot.is_some() {
            return Err(RegisterError::EmailExists);
        }
        let pilot_id = new_id();
        let member_id = new_id();
        let mut tx = self.pool.begin().await.map_err(RegisterError::Db)?;
        sqlx::query(
            "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, 'pending', 'pilot', $7, $8)",
        )
        .bind(&pilot_id)
        .bind(club_id)
        .bind(pilot_account_id.as_deref())
        .bind(name.trim())
        .bind(&email)
        .bind(license_no.trim())
        .bind(pilot_password_hash)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(RegisterError::Db)?;
        sqlx::query(
            "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
             VALUES ($1, $2, $3, 'pilot', 'pending', $4)",
        )
        .bind(&member_id)
        .bind(&pilot_id)
        .bind(club_id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(RegisterError::Db)?;
        tx.commit().await.map_err(RegisterError::Db)?;
        Ok((pilot_id, member_id))
    }

    /// Compat: mint or reuse `pilot_sessions` for the active pilot (switch-club / legacy callers).
    #[allow(dead_code)] // exercised by integration tests; not used on hot viewer path (N-6).
    pub async fn pilot_session_for_account(
        &self,
        account_session_id: &str,
        now: i64,
    ) -> Option<(String, PilotRow)> {
        let session_db_id = auth::digest_token(account_session_id);
        let (_, active_mem) = self.account_from_session(account_session_id, now).await?;
        let mem_id = active_mem?;
        let row =
            sqlx::query("SELECT m.pilot_id, m.status FROM club_memberships m WHERE m.id = $1")
                .bind(&mem_id)
                .fetch_optional(&self.pool)
                .await
                .ok()??;
        let pilot_id: String = row.try_get(0).ok()?;
        let mem_status: String = row.try_get(1).ok()?;
        if !MembershipStatus::parse(&mem_status).allows_club_api() {
            return None;
        }
        if let Some(reuse_hash) = sqlx::query_scalar::<_, Option<String>>(
            "SELECT compat_pilot_session FROM account_sessions WHERE id = $1",
        )
        .bind(&session_db_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .flatten()
        {
            if let Some(pilot) = self.pilot_from_pilot_session_db_id(&reuse_hash, now).await {
                if pilot.id == pilot_id {
                    return Some((String::new(), pilot));
                }
            }
        }
        let session = self.create_pilot_session(&pilot_id, now).await.ok()?;
        let pilot_session_db_id = auth::digest_token(&session);
        let _ = sqlx::query("UPDATE account_sessions SET compat_pilot_session = $1 WHERE id = $2")
            .bind(&pilot_session_db_id)
            .bind(&session_db_id)
            .execute(&self.pool)
            .await;
        let pilot = self.pilot_from_session(&session, now).await?;
        Some((session, pilot))
    }

    pub async fn membership_club_id(&self, membership_id: &str) -> Result<Option<String>, DbError> {
        sqlx::query_scalar("SELECT club_id FROM club_memberships WHERE id = $1")
            .bind(membership_id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn approve_membership(&self, membership_id: &str, now: i64) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT m.pilot_id, m.club_id, m.status, a.email_verified_at, p.email_verified_at
             FROM club_memberships m
             JOIN pilots p ON p.id = m.pilot_id
             LEFT JOIN accounts a ON a.id = p.account_id
             WHERE m.id = $1
             FOR UPDATE OF m",
        )
        .bind(membership_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Err(DbError::protocol("missing membership"));
        };
        let pilot_id: String = row.try_get(0)?;
        let club_id: String = row.try_get(1)?;
        let cur_status: String = row.try_get(2)?;
        let account_verified: Option<i64> = row.try_get(3).ok().flatten();
        let pilot_verified: Option<i64> = row.try_get(4).ok().flatten();
        if cur_status != "pending" {
            return Err(DbError::protocol("membership not pending"));
        }
        if account_verified.is_none() || pilot_verified.is_none() {
            return Err(DbError::protocol("email not verified"));
        }
        let updated = sqlx::query(
            "UPDATE club_memberships SET status = 'active' WHERE id = $1 AND status = 'pending'",
        )
        .bind(membership_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::protocol("membership not pending"));
        }
        sqlx::query(
            "UPDATE pilots SET status = 'approved', role = role, password_hash = '' WHERE id = $1",
        )
        .bind(&pilot_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        let _ = (club_id, now);
        Ok(())
    }

    pub async fn set_account_suspended(&self, account_id: &str, now: i64) -> Result<bool, DbError> {
        let r = sqlx::query(
            "UPDATE accounts SET suspended_at = $1 WHERE id = $2 AND suspended_at IS NULL",
        )
        .bind(now)
        .bind(account_id)
        .execute(&self.pool)
        .await?;
        if r.rows_affected() > 0 {
            sqlx::query("DELETE FROM account_sessions WHERE account_id = $1")
                .bind(account_id)
                .execute(&self.pool)
                .await?;
            sqlx::query(
                "DELETE FROM pilot_sessions WHERE pilot_id IN (SELECT id FROM pilots WHERE account_id = $1)",
            )
            .bind(account_id)
            .execute(&self.pool)
            .await?;
        }
        Ok(r.rows_affected() > 0)
    }

    pub async fn clear_account_suspended(&self, account_id: &str) -> Result<bool, DbError> {
        let r = sqlx::query(
            "UPDATE accounts SET suspended_at = NULL WHERE id = $1 AND suspended_at IS NOT NULL",
        )
        .bind(account_id)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() > 0)
    }

    #[allow(dead_code)]
    pub async fn suspend_membership(&self, membership_id: &str) -> Result<bool, DbError> {
        let row = sqlx::query("SELECT pilot_id FROM club_memberships WHERE id = $1")
            .bind(membership_id)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        let pilot_id: String = row.try_get(0)?;
        let r = sqlx::query(
            "UPDATE club_memberships SET status = 'suspended' WHERE id = $1 AND status <> 'suspended'",
        )
        .bind(membership_id)
        .execute(&self.pool)
        .await?;
        if r.rows_affected() > 0 {
            let _ = sqlx::query(
                "UPDATE pilots SET status = 'suspended' WHERE id = $1 AND status = 'approved'",
            )
            .bind(&pilot_id)
            .execute(&self.pool)
            .await;
            let _ = self.revoke_pilot_live_sessions(&pilot_id).await;
        }
        Ok(r.rows_affected() > 0)
    }

    /// Idempotent: ensure Pablo-style account + CAS admin membership.
    pub async fn seed_account_cli(
        &self,
        email: &str,
        club_slug: &str,
        role: &str,
        dry_run: bool,
    ) -> Result<serde_json::Value, DbError> {
        let club_id: Option<String> = sqlx::query_scalar("SELECT id FROM clubs WHERE slug = $1")
            .bind(club_slug)
            .fetch_optional(&self.pool)
            .await?;
        let Some(club_id) = club_id else {
            return Ok(serde_json::json!({ "error": "club not found", "dryRun": dry_run }));
        };
        let existing_account = self.account_by_email(email).await;
        let account_exists = existing_account.is_some();
        let account_id = existing_account
            .as_ref()
            .map(|a| a.id.clone())
            .unwrap_or_else(new_id);
        let mut plan = serde_json::json!({
            "dryRun": dry_run,
            "email": email,
            "clubSlug": club_slug,
            "accountId": account_id,
            "actions": [],
        });
        if !account_exists {
            plan["actions"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!("create account"));
        }
        let pilot: Option<String> = sqlx::query_scalar(
            "SELECT p.id FROM pilots p
             JOIN accounts a ON a.id = p.account_id
             WHERE lower(a.email) = lower($1) AND p.club_id = $2",
        )
        .bind(email)
        .bind(&club_id)
        .fetch_optional(&self.pool)
        .await?;
        let membership_role = if membership_role_is_admin(role) {
            "admin"
        } else {
            role
        };
        if pilot.is_none() {
            plan["actions"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!("create pilot + membership"));
        } else {
            plan["actions"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!("ensure active membership"));
        }
        if dry_run {
            return Ok(plan);
        }
        let now = now_ms();
        if !account_exists {
            sqlx::query(
                "INSERT INTO accounts (id, email, created_at, email_verified_at)
                 VALUES ($1, $2, $3, $3)",
            )
            .bind(&account_id)
            .bind(email.trim().to_ascii_lowercase())
            .bind(now)
            .execute(&self.pool)
            .await?;
        }
        let pilot_id = if let Some(pid) = pilot {
            pid
        } else {
            let pid = new_id();
            sqlx::query(
                "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
                 VALUES ($1, $2, $3, $4, $5, '', 'approved', $6, '', $7)",
            )
            .bind(&pid)
            .bind(&club_id)
            .bind(&account_id)
            .bind("Admin")
            .bind(email.trim().to_ascii_lowercase())
            .bind(membership_role)
            .bind(now)
            .execute(&self.pool)
            .await?;
            let mid = new_id();
            sqlx::query(
                "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
                 VALUES ($1, $2, $3, $4, 'active', $5)
                 ON CONFLICT(pilot_id, club_id) DO UPDATE SET role = excluded.role, status = 'active'",
            )
            .bind(&mid)
            .bind(&pid)
            .bind(&club_id)
            .bind(membership_role)
            .bind(now)
            .execute(&self.pool)
            .await?;
            pid
        };
        sqlx::query(
            "UPDATE club_memberships SET status = 'active', role = $1
             WHERE pilot_id = $2 AND club_id = $3",
        )
        .bind(membership_role)
        .bind(&pilot_id)
        .bind(&club_id)
        .execute(&self.pool)
        .await?;
        sqlx::query("UPDATE pilots SET status = 'approved', role = $1 WHERE id = $2")
            .bind(membership_role)
            .bind(&pilot_id)
            .execute(&self.pool)
            .await?;
        plan["pilotId"] = serde_json::json!(pilot_id);
        plan["applied"] = serde_json::json!(true);
        Ok(plan)
    }
}

fn row_to_account(row: &sqlx::postgres::PgRow) -> AccountRow {
    AccountRow {
        id: row.try_get(0).unwrap_or_default(),
        email: row.try_get(1).unwrap_or_default(),
        email_verified_at: row.try_get(2).ok().flatten(),
        global_role: row.try_get(3).unwrap_or_default(),
        suspended_at: row.try_get(4).ok().flatten(),
        locale: row.try_get(5).ok().flatten(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::club::auth::hash_password;
    use serial_test::serial;

    #[serial]
    #[tokio::test]
    async fn migration_015_file_then_rerun_016_017() {
        let db = ClubDb::open_test().await.unwrap();
        let pilot_id = new_id();
        let club_id = db.default_club_id().await.unwrap();
        let now = now_ms();
        sqlx::query(
            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
             VALUES ($1, $2, 'P', 'mig-015@test', '', 'approved', 'pilot', '', $3)",
        )
        .bind(&pilot_id)
        .bind(&club_id)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM club_memberships")
            .execute(&db.pool)
            .await
            .unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("migrations/000015_backfill_club_memberships.sql");
        let sql = std::fs::read_to_string(&path).unwrap();
        sqlx::query(&sql).execute(&db.pool).await.unwrap();
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM club_memberships WHERE pilot_id = $1")
                .bind(&pilot_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(n, 1);
        crate::club::database::run_migrations(&db.pool)
            .await
            .expect("016/017 after 015");
        let locale_nullable: bool = sqlx::query_scalar(
            "SELECT is_nullable = 'YES' FROM information_schema.columns
             WHERE table_name = 'pilots' AND column_name = 'locale'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert!(locale_nullable);
    }

    #[serial]
    #[tokio::test]
    async fn migrations_015_through_017_fresh_and_idempotent() {
        let db = ClubDb::open_test().await.unwrap();
        let memberships: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM club_memberships")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert!(memberships >= 0);
        let accounts: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM information_schema.columns
                WHERE table_name = 'accounts' AND column_name = 'locale'
            )",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert!(accounts);
        let pilot_locale_nullable: bool = sqlx::query_scalar(
            "SELECT is_nullable = 'YES' FROM information_schema.columns
             WHERE table_name = 'pilots' AND column_name = 'locale'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert!(pilot_locale_nullable);
        crate::club::database::run_migrations(&db.pool)
            .await
            .expect("re-run migrations");
    }

    #[serial]
    #[tokio::test]
    async fn migration_accounts_idempotent_twice() {
        let db = ClubDb::open_test().await.unwrap();
        let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        crate::club::database::run_migrations(&db.pool)
            .await
            .expect("re-run migrations");
        let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(before, after);
    }

    #[serial]
    #[tokio::test]
    async fn same_email_two_clubs_one_account() {
        let db = ClubDb::open_test().await.unwrap();
        let club_a = db.default_club_id().await.unwrap();
        let club_b = {
            let cid = new_id();
            sqlx::query(
                "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at)
                 VALUES ($1, 'B Club', 'b-club', $2, 'live', $2)",
            )
            .bind(&cid)
            .bind(now_ms())
            .execute(&db.pool)
            .await
            .unwrap();
            cid
        };
        let email = "multi@test.local";
        let hash = hash_password("password12").unwrap();
        let now = now_ms();
        let aid = db.ensure_account_for_email(email, None, now).await.unwrap();
        sqlx::query("UPDATE accounts SET email_verified_at = $1 WHERE id = $2")
            .bind(now)
            .bind(&aid)
            .execute(&db.pool)
            .await
            .unwrap();
        let sess = db.create_account_session(&aid, None, now).await.unwrap();
        db.set_account_password(&aid, "password12", now, None, Some(&sess))
            .await
            .unwrap();
        for (club, name) in [(&club_a, "A"), (&club_b, "B")] {
            let pid = new_id();
            sqlx::query(
                "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
                 VALUES ($1, $2, $3, $4, $5, '', 'approved', 'pilot', $6, $7)",
            )
            .bind(&pid)
            .bind(club)
            .bind(&aid)
            .bind(name)
            .bind(email)
            .bind(&hash)
            .bind(now)
            .execute(&db.pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
                 VALUES ($1, $2, $3, 'pilot', 'active', $4) ON CONFLICT DO NOTHING",
            )
            .bind(new_id())
            .bind(&pid)
            .bind(club)
            .bind(now)
            .execute(&db.pool)
            .await
            .unwrap();
        }
        let accounts: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT account_id) FROM pilots WHERE lower(email) = lower($1)",
        )
        .bind(email)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(accounts, 1);
        let mems = db.list_account_memberships(&aid).await;
        assert_eq!(mems.len(), 2);
    }

    #[serial]
    #[tokio::test]
    async fn magic_send_throttle_per_email() {
        let db = ClubDb::open_test().await.unwrap();
        let now = now_ms();
        let email = "throttle-magic@test";
        let ip = "203.0.113.77";
        for _ in 0..crate::club::db_auth_pg::MAGIC_SEND_MAX_PER_EMAIL {
            let _ = db.log_magic_send(email, ip, now).await;
        }
        assert!(!db.magic_send_allowed(email, ip, now).await);
    }

    #[serial]
    #[tokio::test]
    async fn ensure_account_idempotent_random_id() {
        let db = ClubDb::open_test().await.unwrap();
        let now = now_ms();
        let email = "rand-id@test";
        let id1 = db
            .ensure_account_for_email(email, Some("password12"), now)
            .await
            .unwrap();
        let id2 = db
            .ensure_account_for_email(email, Some("password12"), now)
            .await
            .unwrap();
        assert_eq!(id1, id2);
        assert_ne!(id1, email);
        assert_eq!(id1.len(), 32);
    }

    #[serial]
    #[tokio::test]
    async fn seed_account_cli_idempotent() {
        let db = ClubDb::open_test().await.unwrap();
        let club_slug = std::env::var("FLIGHT_CLUB_SLUG").unwrap_or_else(|_| "default".to_string());
        let dry = db
            .seed_account_cli("pablo@test", &club_slug, "admin", true)
            .await
            .unwrap();
        assert_eq!(dry["dryRun"], true);
        let apply = db
            .seed_account_cli("pablo@test", &club_slug, "admin", false)
            .await
            .unwrap();
        assert_eq!(apply["applied"], true);
        let again = db
            .seed_account_cli("pablo@test", &club_slug, "admin", false)
            .await
            .unwrap();
        assert!(again.get("applied").is_some());
    }
}
