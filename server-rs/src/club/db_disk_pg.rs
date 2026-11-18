//! Disk library slug ownership (public slug vs on-disk storage slug).

use crate::club::{
    db::{now_ms, ClubDb, DbError},
    disk_owner::{INSERT_DISK_FLIGHT_OWNER, INSERT_DISK_LIVE_SESSION_OWNER, OWNER_COL},
};

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

    /// Whether `owner_id` owns the public disk slug (session pilot id until accounts land).
    pub async fn owner_owns_disk_slug(&self, owner_id: &str, public_slug: &str) -> bool {
        let sql = format!(
            "SELECT EXISTS(SELECT 1 FROM disk_flight_owners WHERE public_slug = $1 AND {OWNER_COL} = $2)",
        );
        sqlx::query_scalar(&sql)
            .bind(public_slug)
            .bind(owner_id)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(false)
    }

    pub async fn pilot_owns_disk_slug(&self, pilot_id: &str, public_slug: &str) -> bool {
        self.owner_owns_disk_slug(pilot_id, public_slug).await
    }

    pub async fn list_disk_public_slugs_for_owner(&self, owner_id: &str) -> Vec<String> {
        let sql =
            format!("SELECT public_slug FROM disk_flight_owners WHERE {OWNER_COL} = $1 ORDER BY public_slug");
        sqlx::query_scalar(&sql)
            .bind(owner_id)
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default()
    }

    pub async fn list_disk_public_slugs_for_pilot(&self, pilot_id: &str) -> Vec<String> {
        self.list_disk_public_slugs_for_owner(pilot_id).await
    }

    pub async fn upsert_disk_flight_owner(
        &self,
        public_slug: &str,
        storage_slug: &str,
        owner_id: &str,
        club_id: &str,
    ) -> Result<bool, DbError> {
        let now = now_ms();
        let r = sqlx::query(INSERT_DISK_FLIGHT_OWNER)
            .bind(public_slug)
            .bind(storage_slug)
            .bind(owner_id)
            .bind(club_id)
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    pub async fn count_disk_flight_owners_for_owner(&self, owner_id: &str) -> i64 {
        let sql = format!("SELECT COUNT(*) FROM disk_flight_owners WHERE {OWNER_COL} = $1");
        sqlx::query_scalar(&sql)
            .bind(owner_id)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0)
    }

    pub async fn count_disk_flight_owners_for_pilot(&self, pilot_id: &str) -> i64 {
        self.count_disk_flight_owners_for_owner(pilot_id).await
    }

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

    pub async fn upsert_live_session_owner(
        &self,
        session_id: &str,
        owner_id: &str,
        club_id: &str,
        device_label: &str,
        fix_count: i32,
        log_bytes: i64,
    ) -> Result<bool, DbError> {
        let now = now_ms();
        let r = sqlx::query(INSERT_DISK_LIVE_SESSION_OWNER)
            .bind(session_id)
            .bind(owner_id)
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