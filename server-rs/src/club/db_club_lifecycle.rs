//! Club lifecycle: self-serve signup, verification, suspension, public listing.

use crate::club::{
    auth::{digest_token, new_id, new_share_token},
    db::{ClubDb, DbError},
    slug::{normalize_slug, validate_slug},
};
use serde::Serialize;
use sqlx::{postgres::PgTransaction, Row};
use std::collections::BTreeSet;

pub const CLUB_SIGNUP_PURPOSE: &str = "club_verify";
const MAGIC_TTL_MS: i64 = 15 * 60 * 1000;
/// Unverified self-serve clubs older than this are purged so the slug can be reused.
pub const PENDING_CLUB_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClubDeleteTombstone {
    pub id: String,
    pub club_id: String,
    pub name: String,
    pub slugs: Vec<String>,
    pub actor_account_id: Option<String>,
    pub deleted_at: i64,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClubDeleteError {
    NotFound,
    SlugMismatch,
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

    pub async fn list_club_delete_tombstones(&self) -> Result<Vec<ClubDeleteTombstone>, DbError> {
        let rows = sqlx::query(
            "SELECT t.id,
                    t.club_id,
                    t.club_name,
                    t.actor_account_id,
                    t.deleted_at,
                    COALESCE(
                        ARRAY_AGG(s.slug ORDER BY s.slug ASC) FILTER (WHERE s.slug IS NOT NULL),
                        ARRAY[]::text[]
                    ) AS slugs
             FROM club_delete_tombstones t
             LEFT JOIN club_delete_tombstone_slugs s ON s.tombstone_id = t.id
             GROUP BY t.id, t.club_id, t.club_name, t.actor_account_id, t.deleted_at
             ORDER BY t.deleted_at DESC, t.id DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(ClubDeleteTombstone {
                id: r.try_get(0)?,
                club_id: r.try_get(1)?,
                name: r.try_get(2)?,
                actor_account_id: r.try_get(3)?,
                deleted_at: r.try_get(4)?,
                slugs: r.try_get(5)?,
            });
        }
        Ok(out)
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

    pub async fn pilot_account_id(&self, pilot_id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT account_id FROM pilots WHERE id = $1")
            .bind(pilot_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .flatten()
    }

    /// Pick another active membership on an operational club after a club is deleted.
    /// Prefers an `admin` membership when multiple operational clubs remain.
    pub async fn next_operational_membership(
        &self,
        account_id: &str,
    ) -> Option<crate::club::db_account::MembershipRow> {
        let memberships = self.list_account_memberships(account_id).await;
        let mut fallback = None;
        for m in memberships {
            if !crate::club::membership_status::MembershipStatus::parse(&m.status).allows_club_api()
            {
                continue;
            }
            if self.club_operational(&m.club_id).await.is_err() {
                continue;
            }
            if m.role == "admin" {
                return Some(m);
            }
            if fallback.is_none() {
                fallback = Some(m);
            }
        }
        fallback
    }

    /// Permanently hard-delete a club and all of its club-scoped rows in one Postgres transaction.
    ///
    /// Before deleting any club-scoped rows, inserts a platform-level tombstone
    /// (`club_delete_tombstones` + `club_delete_tombstone_slugs`) recording the club id, name,
    /// current + historical slugs, actor account id, and deletion timestamp. Those slugs remain
    /// blocked from re-registration, rename, and confusable lookalikes for
    /// [`crate::club::db_club_slug::CLUB_SLUG_DELETE_COOLDOWN_MS`] (90 days), unlike `purge_stale_pending_clubs` which
    /// releases unverified pending slugs immediately without writing a tombstone.
    ///
    /// Soft vs hard delete:
    /// - `clubs.suspended_at` is reserved for reversible platform-operator suspension.
    /// - `clubs.status` only models `live` / `pending`; adding a `deleted` state would require
    ///   changes across every operational/signup/session check.
    /// - Hard-deleting in a single transaction with an atomic tombstone cleans up club PII while
    ///   preserving a platform audit record and slug cooldown.
    ///
    /// Club-scoped tables deleted (in FK-safe child-first order):
    /// `fixes`, `flight_devices`, `flight_shares`, `flights`, `checkins`,
    /// `checkin_handoff_tokens` (no FK), `checkouts`, `device_pair_tokens`, `devices`,
    /// `admin_map_prefs`, `access_requests`, `audit_log`,
    /// `magic_login_tokens` (club/pilot-scoped), `guest_sessions`, `aircraft`, `aircraft_types`,
    /// `pilot_sessions` (invalidates `fv_club_session`), `webauthn_credentials`,
    /// `club_memberships` (`account_sessions.active_membership_id` falls back via `ON DELETE SET NULL`),
    /// `pilots`, `club_slug_history`, `club_slug_registry`, and `clubs`.
    ///
    /// Preserved global/platform tables: `accounts`, `account_sessions`, `email_suppressions`,
    /// `login_attempts`, `magic_send_log`, `club_delete_tombstones`,
    /// `club_delete_tombstone_slugs`, and all other clubs' rows.
    ///
    /// Security: callers must enforce exact JSON `Content-Type`, `csrf::cookie_post_allowed`,
    /// active-club-only scoping (never a club id from request input), and
    /// `require_active_club_admin_role`. Re-validates the normalized club slug under `FOR UPDATE`.
    pub async fn delete_club(
        &self,
        club_id: &str,
        confirm_slug: &str,
        actor_account_id: Option<&str>,
        now: i64,
    ) -> Result<(), ClubDeleteError> {
        let mut tx = self.pool.begin().await.map_err(|_| ClubDeleteError::Db)?;
        match Self::delete_club_in_tx(&mut tx, club_id, confirm_slug, actor_account_id, now).await {
            Ok(()) => {
                tx.commit().await.map_err(|_| ClubDeleteError::Db)?;
                Ok(())
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }

    async fn delete_club_in_tx(
        tx: &mut PgTransaction<'_>,
        club_id: &str,
        confirm_slug: &str,
        actor_account_id: Option<&str>,
        now: i64,
    ) -> Result<(), ClubDeleteError> {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(club_id)
            .execute(&mut **tx)
            .await
            .map_err(|_| ClubDeleteError::Db)?;

        let club_row = sqlx::query("SELECT slug, name FROM clubs WHERE id = $1 FOR UPDATE")
            .bind(club_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| ClubDeleteError::Db)?;
        let Some(club_row) = club_row else {
            return Err(ClubDeleteError::NotFound);
        };
        let current_slug: String = club_row.try_get(0).map_err(|_| ClubDeleteError::Db)?;
        let club_name: String = club_row.try_get(1).map_err(|_| ClubDeleteError::Db)?;
        let expected = normalize_slug(&current_slug);
        let provided = normalize_slug(confirm_slug);
        if expected.is_empty() || provided != expected {
            return Err(ClubDeleteError::SlugMismatch);
        }

        // Collect current + historical slugs and record platform tombstone before any DELETE.
        let historical_slugs: Vec<String> = sqlx::query_scalar(
            "SELECT old_slug FROM club_slug_history WHERE club_id = $1 ORDER BY old_slug ASC",
        )
        .bind(club_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        let mut tombstone_slugs = BTreeSet::new();
        tombstone_slugs.insert(expected);
        for old in historical_slugs {
            let norm = normalize_slug(&old);
            if !norm.is_empty() {
                tombstone_slugs.insert(norm);
            }
        }

        let tombstone_id = new_id();
        sqlx::query(
            "INSERT INTO club_delete_tombstones (id, club_id, club_name, actor_account_id, deleted_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&tombstone_id)
        .bind(club_id)
        .bind(&club_name)
        .bind(actor_account_id)
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        for slug in &tombstone_slugs {
            sqlx::query(
                "INSERT INTO club_delete_tombstone_slugs (tombstone_id, slug)
                 VALUES ($1, $2)",
            )
            .bind(&tombstone_id)
            .bind(slug)
            .execute(&mut **tx)
            .await
            .map_err(|_| ClubDeleteError::Db)?;
        }

        // Flight telemetry, device links, shares, and flights (including legacy rows where club_id IS NULL).
        sqlx::query(
            "DELETE FROM fixes
             WHERE flight_id IN (
                 SELECT id FROM flights
                 WHERE club_id = $1
                    OR pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)
                    OR aircraft_id IN (SELECT id FROM aircraft WHERE club_id = $1)
             )
             OR device_id IN (
                 SELECT id FROM devices WHERE pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)
             )",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query(
            "DELETE FROM flight_devices
             WHERE flight_id IN (
                 SELECT id FROM flights
                 WHERE club_id = $1
                    OR pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)
                    OR aircraft_id IN (SELECT id FROM aircraft WHERE club_id = $1)
             )
             OR device_id IN (
                 SELECT id FROM devices WHERE pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)
             )",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query(
            "DELETE FROM flight_shares
             WHERE club_id = $1
                OR flight_id IN (
                    SELECT id FROM flights
                    WHERE club_id = $1
                       OR pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)
                       OR aircraft_id IN (SELECT id FROM aircraft WHERE club_id = $1)
                )
                OR created_by IN (SELECT id FROM pilots WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query(
            "DELETE FROM flights
             WHERE club_id = $1
                OR pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)
                OR aircraft_id IN (SELECT id FROM aircraft WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        // Check-ins and check-in handoff tokens (checkin_handoff_tokens has no FK).
        sqlx::query(
            "DELETE FROM checkins
             WHERE club_id = $1
                OR pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)
                OR device_id IN (
                    SELECT id FROM devices WHERE pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)
                )",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query(
            "DELETE FROM checkin_handoff_tokens
             WHERE club_id = $1
                OR pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        // Type ratings / checkouts.
        sqlx::query(
            "DELETE FROM checkouts
             WHERE club_id = $1
                OR pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)
                OR type_id IN (SELECT id FROM aircraft_types WHERE club_id = $1)
                OR aircraft_id IN (SELECT id FROM aircraft WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        // Devices, pairing tokens, and admin map preferences.
        sqlx::query(
            "DELETE FROM device_pair_tokens
             WHERE device_id IN (
                 SELECT id FROM devices WHERE pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)
             )",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query(
            "DELETE FROM devices
             WHERE pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query(
            "DELETE FROM admin_map_prefs
             WHERE pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        // Access requests and audit log (null out cross-club actor references without deleting other clubs' rows).
        sqlx::query(
            "UPDATE access_requests SET decided_by_pilot_id = NULL
             WHERE club_id <> $1
               AND decided_by_pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query(
            "DELETE FROM access_requests
             WHERE club_id = $1
                OR pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)
                OR type_id IN (SELECT id FROM aircraft_types WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query(
            "UPDATE audit_log SET actor_pilot_id = NULL
             WHERE club_id <> $1
               AND actor_pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query("DELETE FROM audit_log WHERE club_id = $1")
            .bind(club_id)
            .execute(&mut **tx)
            .await
            .map_err(|_| ClubDeleteError::Db)?;

        // Club/pilot-scoped magic tokens and guest sessions.
        sqlx::query(
            "DELETE FROM magic_login_tokens
             WHERE club_id = $1
                OR pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query(
            "DELETE FROM guest_sessions
             WHERE club_id = $1
                OR guest_pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        // Fleet.
        sqlx::query("DELETE FROM aircraft WHERE club_id = $1")
            .bind(club_id)
            .execute(&mut **tx)
            .await
            .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query("DELETE FROM aircraft_types WHERE club_id = $1")
            .bind(club_id)
            .execute(&mut **tx)
            .await
            .map_err(|_| ClubDeleteError::Db)?;

        // Pilot sessions (invalidates fv_club_session server-side), passkeys, memberships, and pilots.
        sqlx::query(
            "DELETE FROM pilot_sessions
             WHERE pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query(
            "DELETE FROM webauthn_credentials
             WHERE pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query(
            "UPDATE account_sessions SET compat_pilot_session = NULL
             WHERE active_membership_id IN (SELECT id FROM club_memberships WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query(
            "DELETE FROM club_memberships
             WHERE club_id = $1
                OR pilot_id IN (SELECT id FROM pilots WHERE club_id = $1)",
        )
        .bind(club_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query("DELETE FROM pilots WHERE club_id = $1")
            .bind(club_id)
            .execute(&mut **tx)
            .await
            .map_err(|_| ClubDeleteError::Db)?;

        // Slug history/registry and finally the club row itself.
        sqlx::query("DELETE FROM club_slug_history WHERE club_id = $1")
            .bind(club_id)
            .execute(&mut **tx)
            .await
            .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query("DELETE FROM club_slug_registry WHERE club_id = $1")
            .bind(club_id)
            .execute(&mut **tx)
            .await
            .map_err(|_| ClubDeleteError::Db)?;

        sqlx::query("DELETE FROM clubs WHERE id = $1")
            .bind(club_id)
            .execute(&mut **tx)
            .await
            .map_err(|_| ClubDeleteError::Db)?;

        Ok(())
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
            .club_slug_globally_taken_at(&slug, None, now)
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
        if Self::slug_taken_in_tx(&mut tx, &slug, None, now)
            .await
            .map_err(|_| ClubSignupError::Db)?
            || Self::confusable_taken_in_tx(&mut tx, &slug, None, now)
                .await
                .map_err(|_| ClubSignupError::Db)?
        {
            let _ = tx.rollback().await;
            return Err(ClubSignupError::SlugTaken);
        }
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
