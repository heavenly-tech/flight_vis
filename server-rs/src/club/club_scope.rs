use crate::club::db::{now_ms, ClubDb, DbError};
use crate::club::db_club_lifecycle::ClubPublicRow;
use sqlx::Row;
use std::collections::HashSet;

pub fn checkin_qr_path(slug: &str, token: &str) -> String {
    format!("/c/{slug}/q/{token}")
}

#[derive(Debug, Clone)]
pub struct ClubContext {
    pub club_id: String,
    pub slug: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClubScopeError {
    NotFound,
    Forbidden,
}

impl ClubDb {
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

    /// `pilots.global_role = superadmin` OR allowlisted email on an approved, verified pilot.
    pub async fn is_platform_superadmin(
        &self,
        pilot_id: &str,
        env_allowlist: &HashSet<String>,
    ) -> bool {
        if self.is_superadmin(pilot_id).await {
            return true;
        }
        let row = sqlx::query("SELECT email, email_verified_at, status FROM pilots WHERE id = $1")
            .bind(pilot_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten();
        let Some(row) = row else {
            return false;
        };
        let email: String = row.try_get(0).ok().unwrap_or_default();
        let verified: Option<i64> = row.try_get(1).ok();
        let status: String = row.try_get(2).ok().unwrap_or_default();
        if verified.is_none() || status != "approved" {
            return false;
        }
        crate::club::superadmin::is_superadmin_email(env_allowlist, &email)
    }

    /// Live, email-verified, and not suspended.
    pub fn club_is_operational(row: &ClubPublicRow) -> bool {
        Self::is_public_verified(row)
    }

    pub async fn club_operational(&self, club_id: &str) -> Result<(), ClubScopeError> {
        let Some(row) = self.club_public_row(club_id).await else {
            return Err(ClubScopeError::NotFound);
        };
        if !Self::club_is_operational(&row) {
            return Err(ClubScopeError::Forbidden);
        }
        Ok(())
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
        self.club_operational(club_id).await?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolve_club_context_unknown_slug() {
        let db = ClubDb::open_test().await.expect("db");
        assert!(matches!(
            db.resolve_club_context("no-such-club-slug-xyz").await,
            Err(ClubScopeError::NotFound)
        ));
    }

    #[tokio::test]
    async fn require_club_admin_forbids_when_not_admin() {
        let db = ClubDb::open_test().await.expect("db");
        let club = db.default_club_id().await.expect("club");
        assert!(matches!(
            db.require_club_admin("nonexistent-pilot", &club).await,
            Err(ClubScopeError::Forbidden)
        ));
    }
}
