//! Club slug rename, historical slugs, and global slug reservation.

use crate::club::{
    auth::new_id,
    db::{ClubDb, DbError},
    slug::{confusable_slug_skeleton, normalize_slug, validate_slug},
};
use sqlx::{postgres::PgTransaction, Row};

pub const MAX_SLUG_RENAMES_PER_CLUB_PER_DAY: i64 = 10;
pub const MAX_SLUG_HISTORY_PER_CLUB: i64 = 50;
const RENAME_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClubSlugRenameError {
    InvalidSlug,
    Taken,
    Unchanged,
    Suspended,
    RateLimited,
    Db,
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(|e| e.code())
        .is_some_and(|c| c == "23505")
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

    async fn slug_taken_in_tx(
        tx: &mut PgTransaction<'_>,
        slug: &str,
        except_club_id: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let slug = normalize_slug(slug);
        let in_registry: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM club_slug_registry
             WHERE slug = $1 AND ($2::text IS NULL OR club_id <> $2)",
        )
        .bind(&slug)
        .bind(except_club_id)
        .fetch_one(&mut **tx)
        .await?;
        Ok(in_registry > 0)
    }

    async fn confusable_taken_in_tx(
        tx: &mut PgTransaction<'_>,
        slug: &str,
        except_club_id: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let skel = confusable_slug_skeleton(slug);
        if skel.is_empty() {
            return Ok(false);
        }
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT slug FROM club_slug_registry WHERE ($1::text IS NULL OR club_id <> $1)",
        )
        .bind(except_club_id)
        .fetch_all(&mut **tx)
        .await?;
        Ok(rows.iter().any(|s| {
            confusable_slug_skeleton(s) == skel && normalize_slug(s) != normalize_slug(slug)
        }))
    }

    pub async fn club_slug_globally_taken(
        &self,
        slug: &str,
        except_club_id: Option<&str>,
    ) -> Result<bool, DbError> {
        let slug = normalize_slug(slug);
        let in_registry: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM club_slug_registry
             WHERE slug = $1 AND ($2::text IS NULL OR club_id <> $2)",
        )
        .bind(&slug)
        .bind(except_club_id)
        .fetch_one(&self.pool)
        .await?;
        if in_registry > 0 {
            return Ok(true);
        }
        self.club_slug_confusable_taken(&slug, except_club_id).await
    }

    pub async fn club_slug_confusable_taken(
        &self,
        slug: &str,
        except_club_id: Option<&str>,
    ) -> Result<bool, DbError> {
        let skel = confusable_slug_skeleton(slug);
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT slug FROM club_slug_registry WHERE ($1::text IS NULL OR club_id <> $1)",
        )
        .bind(except_club_id)
        .fetch_all(&self.pool)
        .await?;
        let norm = normalize_slug(slug);
        Ok(rows
            .iter()
            .any(|s| confusable_slug_skeleton(s) == skel && normalize_slug(s) != norm))
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
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| ClubSlugRenameError::Db)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(club_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ClubSlugRenameError::Db)?;
        let row = sqlx::query("SELECT slug, suspended_at FROM clubs WHERE id = $1 FOR UPDATE")
            .bind(club_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| ClubSlugRenameError::Db)?;
        let Some(row) = row else {
            let _ = tx.rollback().await;
            return Err(ClubSlugRenameError::Db);
        };
        let old_slug: String = row.try_get(0).map_err(|_| ClubSlugRenameError::Db)?;
        let suspended_at: Option<i64> = row.try_get(1).ok();
        if suspended_at.is_some() {
            let _ = tx.rollback().await;
            return Err(ClubSlugRenameError::Suspended);
        }
        if old_slug == new_slug {
            let _ = tx.rollback().await;
            return Err(ClubSlugRenameError::Unchanged);
        }
        let renames: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log
             WHERE club_id = $1 AND action = 'club.slug.rename' AND created_at > $2",
        )
        .bind(club_id)
        .bind(now - RENAME_WINDOW_MS)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| ClubSlugRenameError::Db)?;
        if renames >= MAX_SLUG_RENAMES_PER_CLUB_PER_DAY {
            let _ = tx.rollback().await;
            return Err(ClubSlugRenameError::RateLimited);
        }
        if Self::slug_taken_in_tx(&mut tx, &new_slug, Some(club_id))
            .await
            .map_err(|_| ClubSlugRenameError::Db)?
            || Self::confusable_taken_in_tx(&mut tx, &new_slug, Some(club_id))
                .await
                .map_err(|_| ClubSlugRenameError::Db)?
        {
            let _ = tx.rollback().await;
            return Err(ClubSlugRenameError::Taken);
        }
        sqlx::query(
            "DELETE FROM club_slug_history
             WHERE club_id = $1 AND old_slug IN (
                 SELECT old_slug FROM club_slug_history WHERE club_id = $1
                 ORDER BY changed_at DESC OFFSET $2
             )",
        )
        .bind(club_id)
        .bind(MAX_SLUG_HISTORY_PER_CLUB - 1)
        .execute(&mut *tx)
        .await
        .map_err(|_| ClubSlugRenameError::Db)?;
        let del_reclaim =
            sqlx::query("DELETE FROM club_slug_history WHERE old_slug = $1 AND club_id = $2")
                .bind(&new_slug)
                .bind(club_id)
                .execute(&mut *tx)
                .await;
        if let Err(e) = del_reclaim {
            if is_unique_violation(&e) {
                let _ = tx.rollback().await;
                return Err(ClubSlugRenameError::Taken);
            }
            let _ = tx.rollback().await;
            return Err(ClubSlugRenameError::Db);
        }
        let ins_hist = sqlx::query(
            "INSERT INTO club_slug_history (old_slug, club_id, changed_at)
             VALUES ($1, $2, $3)",
        )
        .bind(&old_slug)
        .bind(club_id)
        .bind(now)
        .execute(&mut *tx)
        .await;
        if let Err(e) = ins_hist {
            if is_unique_violation(&e) {
                let _ = tx.rollback().await;
                return Err(ClubSlugRenameError::Taken);
            }
            let _ = tx.rollback().await;
            return Err(ClubSlugRenameError::Db);
        }
        let upd = sqlx::query("UPDATE clubs SET slug = $1 WHERE id = $2")
            .bind(&new_slug)
            .bind(club_id)
            .execute(&mut *tx)
            .await;
        if let Err(e) = upd {
            if is_unique_violation(&e) {
                let _ = tx.rollback().await;
                return Err(ClubSlugRenameError::Taken);
            }
            let _ = tx.rollback().await;
            return Err(ClubSlugRenameError::Db);
        }
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
