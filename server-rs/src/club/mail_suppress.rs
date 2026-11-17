#![allow(dead_code)]

use crate::club::db::{now_ms, ClubDb, DbError};

impl ClubDb {
    pub async fn suppress_email(&self, email: &str, reason: &str) -> Result<(), DbError> {
        let email = email.trim().to_ascii_lowercase();
        sqlx::query(
            "INSERT INTO email_suppressions (email, reason, created_at)
             VALUES ($1, $2, $3)
             ON CONFLICT(email) DO UPDATE SET reason = excluded.reason, created_at = excluded.created_at",
        )
        .bind(email)
        .bind(reason)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn email_suppressed(&self, email: &str) -> bool {
        let email = email.trim().to_ascii_lowercase();
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM email_suppressions WHERE email = $1")
            .bind(email)
            .fetch_one(&self.pool)
            .await
            .map(|n| n > 0)
            .unwrap_or(false)
    }
}