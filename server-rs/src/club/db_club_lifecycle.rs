//! Club lifecycle: self-serve signup, verification, suspension, public listing.

use crate::club::{
    auth::{digest_token, new_id, new_share_token},
    db::{ClubDb, DbError},
    slug::{normalize_slug, validate_slug},
};
use sqlx::Row;

pub const CLUB_SIGNUP_PURPOSE: &str = "club_verify";
const MAGIC_TTL_MS: i64 = 15 * 60 * 1000;
/// Unverified self-serve clubs older than this are purged so the slug can be reused.
pub const PENDING_CLUB_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ClubPublicRow {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub status: String,
    pub email_verified_at: Option<i64>,
    pub suspended_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClubRegisterGate {
    Open,
    Suspended,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClubSignupError {
    InvalidSlug,
    SlugTaken,
    InvalidIcao,
    InvalidEmail,
    Db,
}

impl ClubDb {
    pub async fn club_public_row_by_slug(&self, slug: &str) -> Option<ClubPublicRow> {
        let slug = normalize_slug(slug);
        let row = sqlx::query(
            "SELECT id, name, slug, status, email_verified_at, suspended_at FROM clubs WHERE slug = $1",
        )
        .bind(&slug)
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        Some(row_to_public(&row))
    }

    pub async fn club_public_row(&self, club_id: &str) -> Option<ClubPublicRow> {
        let row = sqlx::query(
            "SELECT id, name, slug, status, email_verified_at, suspended_at FROM clubs WHERE id = $1",
        )
        .bind(club_id)
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        Some(row_to_public(&row))
    }

    pub fn register_gate(row: &ClubPublicRow) -> ClubRegisterGate {
        if row.suspended_at.is_some() {
            return ClubRegisterGate::Suspended;
        }
        if row.status != "live" || row.email_verified_at.is_none() {
            return ClubRegisterGate::Closed;
        }
        ClubRegisterGate::Open
    }

    pub fn is_public_verified(row: &ClubPublicRow) -> bool {
        Self::register_gate(row) == ClubRegisterGate::Open
    }

    pub async fn list_public_clubs(&self) -> Vec<(String, String)> {
        sqlx::query(
            "SELECT slug, name FROM clubs
             WHERE status = 'live' AND email_verified_at IS NOT NULL AND suspended_at IS NULL
             ORDER BY lower(name)",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| {
            let slug: String = r.try_get(0).ok()?;
            let name: String = r.try_get(1).ok()?;
            Some((slug, name))
        })
        .collect()
    }

    pub async fn list_all_clubs_admin(&self) -> Vec<serde_json::Value> {
        let rows = sqlx::query(
            "SELECT id, name, slug, status, contact_email, email_verified_at, suspended_at,
                    aerodrome_icao, created_at
             FROM clubs ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(json!({
                    "id": r.try_get::<String, _>(0).ok()?,
                    "name": r.try_get::<String, _>(1).ok()?,
                    "slug": r.try_get::<String, _>(2).ok()?,
                    "status": r.try_get::<String, _>(3).ok()?,
                    "contactEmail": r.try_get::<String, _>(4).ok()?,
                    "emailVerifiedAt": r.try_get::<Option<i64>, _>(5).ok()?,
                    "suspendedAt": r.try_get::<Option<i64>, _>(6).ok()?,
                    "aerodromeIcao": r.try_get::<Option<String>, _>(7).ok()?,
                    "createdAt": r.try_get::<i64, _>(8).ok()?,
                }))
            })
            .collect()
    }

    pub async fn set_club_suspended(&self, club_id: &str, now: i64) -> Result<bool, DbError> {
        let mut tx = self.pool.begin().await?;
        let r = sqlx::query(
            "UPDATE clubs SET suspended_at = $1 WHERE id = $2 AND suspended_at IS NULL",
        )
        .bind(now)
        .bind(club_id)
        .execute(&mut *tx)
        .await?;
        if r.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        sqlx::query(
            "DELETE FROM pilot_sessions
             WHERE pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE devices SET token_hash = 'revoked-suspend-' || id || '-' || $1::text
             WHERE pilot_id IN (SELECT id FROM pilots WHERE club_id = $2)",
        )
        .bind(now)
        .bind(club_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM device_pair_tokens
             WHERE device_id IN (
                 SELECT d.id FROM devices d
                 JOIN pilots p ON p.id = d.pilot_id
                 WHERE p.club_id = $1
             )",
        )
        .bind(club_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn clear_club_suspended(&self, club_id: &str) -> Result<bool, DbError> {
        let r = sqlx::query(
            "UPDATE clubs SET suspended_at = NULL WHERE id = $1 AND suspended_at IS NOT NULL",
        )
        .bind(club_id)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() > 0)
    }

    pub async fn purge_stale_pending_clubs(&self, now: i64) -> Result<u64, DbError> {
        let cutoff = now - PENDING_CLUB_TTL_MS;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "DELETE FROM pilots
             WHERE club_id IN (
                 SELECT id FROM clubs
                 WHERE status = 'pending'
                   AND email_verified_at IS NULL
                   AND created_at < $1
             )",
        )
        .bind(cutoff)
        .execute(&mut *tx)
        .await?;
        let r = sqlx::query(
            "DELETE FROM clubs
             WHERE status = 'pending'
               AND email_verified_at IS NULL
               AND created_at < $1",
        )
        .bind(cutoff)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(r.rows_affected())
    }

    pub async fn signup_club(
        &self,
        name: &str,
        slug: &str,
        contact_email: &str,
        aerodrome_icao: Option<&str>,
        now: i64,
    ) -> Result<(String, String), ClubSignupError> {
        let _ = self.purge_stale_pending_clubs(now).await;
        let name = name.trim();
        let slug = normalize_slug(slug);
        let email = contact_email.trim().to_ascii_lowercase();
        if name.is_empty() || name.chars().count() > 80 || email.is_empty() || !email.contains('@')
        {
            return Err(ClubSignupError::InvalidEmail);
        }
        if validate_slug(&slug).is_err() {
            return Err(ClubSignupError::InvalidSlug);
        }
        if self
            .club_slug_globally_taken(&slug, None)
            .await
            .map_err(|_| ClubSignupError::Db)?
        {
            return Err(ClubSignupError::SlugTaken);
        }
        if let Some(icao) = aerodrome_icao {
            if !icao.trim().is_empty() && !validate_icao(icao) {
                return Err(ClubSignupError::InvalidIcao);
            }
        }
        let club_id = new_id();
        let admin_id = new_id();
        let member_id = new_id();
        let icao = aerodrome_icao
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_uppercase());
        let mut tx = self.pool.begin().await.map_err(|_| ClubSignupError::Db)?;
        let ins = sqlx::query(
            "INSERT INTO clubs (id, name, slug, created_at, status, contact_email, aerodrome_icao,
             checkin_window_hours, latitude, longitude)
             VALUES ($1, $2, $3, $4, 'pending', $5, $6, 12, -33.4489, -70.6693)",
        )
        .bind(&club_id)
        .bind(name)
        .bind(&slug)
        .bind(now)
        .bind(&email)
        .bind(&icao)
        .execute(&mut *tx)
        .await;
        if let Err(e) = ins {
            if let Some(db_err) = e.as_database_error() {
                if db_err.code() == Some(std::borrow::Cow::Borrowed("23505")) {
                    let _ = tx.rollback().await;
                    return Err(ClubSignupError::SlugTaken);
                }
            }
            let _ = tx.rollback().await;
            return Err(ClubSignupError::Db);
        }
        let admin_name = format!("Admin · {name}");
        let account_id = if let Some(a) = self.account_by_email(&email).await {
            a.id
        } else {
            let aid = new_id();
            sqlx::query(
                "INSERT INTO accounts (id, email, created_at)
                 VALUES ($1, $2, $3)",
            )
            .bind(&aid)
            .bind(&email)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| ClubSignupError::Db)?;
            aid
        };
        sqlx::query(
            "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
             VALUES ($1, $2, $3, $4, $5, '', 'pending', 'admin', '', $6)",
        )
        .bind(&admin_id)
        .bind(&club_id)
        .bind(&account_id)
        .bind(&admin_name)
        .bind(&email)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|_| ClubSignupError::Db)?;
        sqlx::query(
            "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
             VALUES ($1, $2, $3, 'admin', 'pending', $4)",
        )
        .bind(&member_id)
        .bind(&admin_id)
        .bind(&club_id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|_| ClubSignupError::Db)?;
        tx.commit().await.map_err(|_| ClubSignupError::Db)?;
        let token = self
            .issue_magic_club_verify(&club_id, &email, now)
            .await
            .map_err(|_| ClubSignupError::Db)?;
        Ok((club_id, token))
    }

    pub async fn issue_magic_club_verify(
        &self,
        club_id: &str,
        email: &str,
        now: i64,
    ) -> Result<String, DbError> {
        let raw_token = new_share_token();
        let token_hash = digest_token(&raw_token);
        let id = new_id();
        let expires = now + MAGIC_TTL_MS;
        sqlx::query(
            "INSERT INTO magic_login_tokens (id, email, token_hash, created_at, expires_at, purpose, club_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&id)
        .bind(email.trim().to_ascii_lowercase())
        .bind(&token_hash)
        .bind(now)
        .bind(expires)
        .bind(CLUB_SIGNUP_PURPOSE)
        .bind(club_id)
        .execute(&self.pool)
        .await?;
        Ok(raw_token)
    }

    pub async fn try_consume_magic_club_verify(
        &self,
        raw_token: &str,
        now: i64,
    ) -> Result<String, crate::club::db::MagicConsumeError> {
        use crate::club::db::MagicConsumeError;
        let token_hash = digest_token(raw_token.trim());
        let row = sqlx::query(
            "SELECT id, club_id, expires_at, used_at, purpose FROM magic_login_tokens WHERE token_hash = $1",
        )
        .bind(&token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| MagicConsumeError::Invalid)?;
        let Some(row) = row else {
            return Err(MagicConsumeError::Invalid);
        };
        let id: String = row.try_get(0).map_err(|_| MagicConsumeError::Invalid)?;
        let club_id: Option<String> = row.try_get(1).ok();
        let expires_at: i64 = row.try_get(2).map_err(|_| MagicConsumeError::Invalid)?;
        let used_at: Option<i64> = row.try_get(3).map_err(|_| MagicConsumeError::Invalid)?;
        let purpose: String = row.try_get(4).unwrap_or_default();
        if purpose != CLUB_SIGNUP_PURPOSE {
            return Err(MagicConsumeError::Invalid);
        }
        let Some(club_id) = club_id else {
            return Err(MagicConsumeError::Invalid);
        };
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
        sqlx::query("UPDATE clubs SET status = 'live', email_verified_at = $1 WHERE id = $2")
            .bind(now)
            .bind(&club_id)
            .execute(&self.pool)
            .await
            .map_err(|_| MagicConsumeError::Invalid)?;
        sqlx::query(
            "UPDATE pilots SET email_verified_at = $1, status = 'approved'
             WHERE club_id = $2 AND role = 'admin'",
        )
        .bind(now)
        .bind(&club_id)
        .execute(&self.pool)
        .await
        .map_err(|_| MagicConsumeError::Invalid)?;
        sqlx::query(
            "UPDATE club_memberships SET status = 'active'
             WHERE club_id = $1 AND role = 'admin'",
        )
        .bind(&club_id)
        .execute(&self.pool)
        .await
        .map_err(|_| MagicConsumeError::Invalid)?;
        sqlx::query(
            "UPDATE accounts AS a SET email_verified_at = coalesce(a.email_verified_at, $1)
             FROM pilots p
             WHERE p.club_id = $2 AND p.role = 'admin' AND a.id = p.account_id",
        )
        .bind(now)
        .bind(&club_id)
        .execute(&self.pool)
        .await
        .map_err(|_| MagicConsumeError::Invalid)?;
        Ok(club_id)
    }
}

pub fn validate_icao(raw: &str) -> bool {
    let s = raw.trim().to_ascii_uppercase();
    s.len() == 4 && s.chars().all(|c| c.is_ascii_alphabetic())
}

fn row_to_public(row: &sqlx::postgres::PgRow) -> ClubPublicRow {
    ClubPublicRow {
        id: row.try_get(0).unwrap_or_default(),
        name: row.try_get(1).unwrap_or_default(),
        slug: row.try_get(2).unwrap_or_default(),
        status: row.try_get(3).unwrap_or_default(),
        email_verified_at: row.try_get::<Option<i64>, _>(4).ok().flatten(),
        suspended_at: row.try_get::<Option<i64>, _>(5).ok().flatten(),
    }
}

use serde_json::json;
