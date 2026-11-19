//! Club slug rename, historical slugs, and global slug reservation.

use crate::club::{
    auth::new_id,
    db::{ClubDb, DbError},
    slug::{normalize_slug, validate_slug},
};
use sqlx::Row;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClubSlugRenameError {
    InvalidSlug,
    Taken,
    Unchanged,
    Db,
}

impl ClubDb {
    pub async fn club_id_for_slug_any(&self, slug: &str) -> Option<String> {
        let slug = normalize_slug(slug);
        if slug.is_empty() {
            return None;
        }
        if let Some(id) = self.club_id_for_slug(&slug).await {
            return Some(id);
        }
        sqlx::query_scalar("SELECT club_id FROM club_slug_history WHERE old_slug = $1")
            .bind(&slug)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    /// `None` if `old_slug` is not a historical club slug (current slugs also return `None`).
    pub async fn current_slug_for_historical(&self, old_slug: &str) -> Option<String> {
        let old_slug = normalize_slug(old_slug);
        if old_slug.is_empty() {
            return None;
        }
        if self.club_id_for_slug(&old_slug).await.is_some() {
            return None;
        }
        sqlx::query_scalar(
            "SELECT c.slug FROM club_slug_history h
             JOIN clubs c ON c.id = h.club_id
             WHERE h.old_slug = $1",
        )
        .bind(&old_slug)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    /// Whether `slug` is blocked for `except_club_id` (signup passes `None`).
    pub async fn club_slug_globally_taken(
        &self,
        slug: &str,
        except_club_id: Option<&str>,
    ) -> Result<bool, DbError> {
        let slug = normalize_slug(slug);
        let in_clubs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM clubs WHERE slug = $1 AND ($2::text IS NULL OR id <> $2)",
        )
        .bind(&slug)
        .bind(except_club_id)
        .fetch_one(&self.pool)
        .await?;
        if in_clubs > 0 {
            return Ok(true);
        }
        let in_hist: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM club_slug_history WHERE old_slug = $1
             AND ($2::text IS NULL OR club_id <> $2)",
        )
        .bind(&slug)
        .bind(except_club_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(in_hist > 0)
    }

    pub async fn rename_club_slug(
        &self,
        club_id: &str,
        new_slug: &str,
        actor_pilot_id: Option<&str>,
        now: i64,
    ) -> Result<String, ClubSlugRenameError> {
        let new_slug = normalize_slug(new_slug);
        if validate_slug(&new_slug).is_err() {
            return Err(ClubSlugRenameError::InvalidSlug);
        }
        let row = sqlx::query("SELECT slug FROM clubs WHERE id = $1 FOR UPDATE")
            .bind(club_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| ClubSlugRenameError::Db)?;
        let Some(row) = row else {
            return Err(ClubSlugRenameError::Db);
        };
        let old_slug: String = row.try_get(0).map_err(|_| ClubSlugRenameError::Db)?;
        if old_slug == new_slug {
            return Err(ClubSlugRenameError::Unchanged);
        }
        if self
            .club_slug_globally_taken(&new_slug, Some(club_id))
            .await
            .map_err(|_| ClubSlugRenameError::Db)?
        {
            return Err(ClubSlugRenameError::Taken);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| ClubSlugRenameError::Db)?;
        if self
            .club_slug_globally_taken(&new_slug, Some(club_id))
            .await
            .map_err(|_| ClubSlugRenameError::Db)?
        {
            let _ = tx.rollback().await;
            return Err(ClubSlugRenameError::Taken);
        }
        sqlx::query("DELETE FROM club_slug_history WHERE old_slug = $1 AND club_id = $2")
            .bind(&new_slug)
            .bind(club_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ClubSlugRenameError::Db)?;
        sqlx::query(
            "INSERT INTO club_slug_history (old_slug, club_id, changed_at)
             VALUES ($1, $2, $3)
             ON CONFLICT (old_slug) DO NOTHING",
        )
        .bind(&old_slug)
        .bind(club_id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|_| ClubSlugRenameError::Db)?;
        sqlx::query("UPDATE clubs SET slug = $1 WHERE id = $2")
            .bind(&new_slug)
            .bind(club_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ClubSlugRenameError::Db)?;
        let detail = format!("{old_slug}→{new_slug}");
        sqlx::query(
            "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
             VALUES ($1, $2, $3, 'club.slug.rename', 'club', $2, $4, $5)",
        )
        .bind(new_id())
        .bind(club_id)
        .bind(actor_pilot_id)
        .bind(&detail)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|_| ClubSlugRenameError::Db)?;
        tx.commit().await.map_err(|_| ClubSlugRenameError::Db)?;
        Ok(old_slug)
    }
}
