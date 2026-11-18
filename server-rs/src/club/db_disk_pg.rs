//! Disk library slug ownership (public slug vs on-disk storage slug).

use crate::club::db::{now_ms, ClubDb, DbError};

impl ClubDb {
    pub async fn disk_storage_slug(&self, public_slug: &str) -> Option<String> {
        sqlx::query_scalar("SELECT storage_slug FROM disk_flight_owners WHERE public_slug = $1")
            .bind(public_slug)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    pub async fn disk_public_slug(&self, storage_slug: &str) -> Option<String> {
        sqlx::query_scalar("SELECT public_slug FROM disk_flight_owners WHERE storage_slug = $1")
            .bind(storage_slug)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    pub async fn pilot_owns_disk_slug(&self, pilot_id: &str, public_slug: &str) -> bool {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM disk_flight_owners WHERE public_slug = $1 AND pilot_id = $2)",
        )
        .bind(public_slug)
        .bind(pilot_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false)
    }

    pub async fn list_disk_public_slugs_for_pilot(&self, pilot_id: &str) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT public_slug FROM disk_flight_owners WHERE pilot_id = $1 ORDER BY public_slug",
        )
        .bind(pilot_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    #[allow(dead_code)]
    pub async fn upsert_disk_flight_owner(
        &self,
        public_slug: &str,
        storage_slug: &str,
        pilot_id: &str,
        club_id: &str,
    ) -> Result<bool, DbError> {
        let now = now_ms();
        let r = sqlx::query(
            "INSERT INTO disk_flight_owners (public_slug, storage_slug, pilot_id, club_id, migrated_at)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (public_slug) DO NOTHING",
        )
        .bind(public_slug)
        .bind(storage_slug)
        .bind(pilot_id)
        .bind(club_id)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() > 0)
    }

    #[allow(dead_code)]
    pub async fn count_disk_flight_owners_for_pilot(&self, pilot_id: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM disk_flight_owners WHERE pilot_id = $1")
            .bind(pilot_id)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0)
    }

    #[allow(dead_code)]
    pub async fn count_unowned_disk_mappings(&self, storage_slugs: &[String]) -> i64 {
        if storage_slugs.is_empty() {
            return 0;
        }
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM unnest($1::text[]) AS s(slug)
             WHERE NOT EXISTS (SELECT 1 FROM disk_flight_owners d WHERE d.storage_slug = s.slug)",
        )
        .bind(storage_slugs)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0)
    }

    #[allow(dead_code)]
    pub async fn upsert_live_session_owner(
        &self,
        session_id: &str,
        pilot_id: &str,
        club_id: &str,
        device_label: &str,
        fix_count: i32,
        log_bytes: i64,
    ) -> Result<bool, DbError> {
        let now = now_ms();
        let r = sqlx::query(
            "INSERT INTO disk_live_session_owners (session_id, pilot_id, club_id, device_label, fix_count, log_bytes, migrated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (session_id) DO NOTHING",
        )
        .bind(session_id)
        .bind(pilot_id)
        .bind(club_id)
        .bind(device_label)
        .bind(fix_count)
        .bind(log_bytes)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() > 0)
    }
}
