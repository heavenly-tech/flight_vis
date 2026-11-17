//! Import `FLIGHT_VIS_DEVICE_TOKENS_FILE` into club `devices` for matching pilots (idempotent).

use crate::club::{
    auth::{digest_token, new_id},
    db::{now_ms, ClubDb, DbError},
};

const IMPORT_LABEL: &str = "imported";

/// Lines: `token=pilot_email` (same shape as live ingest token files; value is pilot email).
pub(crate) fn parse_import_lines(raw: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (token, email) = match line.split_once('=') {
            Some(p) => p,
            None => match line.split_once(|c: char| c.is_whitespace()) {
                Some(p) => p,
                None => continue,
            },
        };
        let token = token.trim();
        let email = email.trim();
        if token.is_empty() || email.is_empty() {
            continue;
        }
        out.push((token.to_string(), email.to_ascii_lowercase()));
    }
    out
}

impl ClubDb {
    pub(crate) async fn import_device_tokens_from_env(&self) -> Result<usize, DbError> {
        let Ok(path_var) = std::env::var("FLIGHT_VIS_DEVICE_TOKENS_FILE") else {
            return Ok(0);
        };
        let path = path_var.trim();
        if path.is_empty() {
            return Ok(0);
        }
        let raw = tokio::fs::read_to_string(path)
            .await
            .map_err(DbError::from)?;
        self.import_device_tokens(&raw).await
    }

    pub async fn import_device_tokens(&self, raw: &str) -> Result<usize, DbError> {
        let entries = parse_import_lines(raw);
        if entries.is_empty() {
            return Ok(0);
        }
        let mut imported = 0usize;
        for (token, email) in entries {
            let pilot_id: Option<String> = sqlx::query_scalar(
                "SELECT id FROM pilots WHERE lower(email) = $1 ORDER BY created_at ASC LIMIT 1",
            )
            .bind(&email)
            .fetch_optional(&self.pool)
            .await?;
            let Some(pilot_id) = pilot_id else {
                continue;
            };
            let hash = digest_token(&token);
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM devices WHERE pilot_id = $1 AND token_hash = $2
                 )",
            )
            .bind(&pilot_id)
            .bind(&hash)
            .fetch_one(&self.pool)
            .await?;
            if exists {
                continue;
            }
            let now = now_ms();
            let id = new_id();
            let inserted = sqlx::query(
                "INSERT INTO devices (id, pilot_id, token_hash, label, created_at)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (token_hash) DO NOTHING",
            )
            .bind(&id)
            .bind(&pilot_id)
            .bind(&hash)
            .bind(IMPORT_LABEL)
            .bind(now)
            .execute(&self.pool)
            .await?;
            if inserted.rows_affected() > 0 {
                imported += 1;
            }
        }
        Ok(imported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[serial]
    #[tokio::test]
    async fn device_token_import_idempotent() {
        let db = ClubDb::open_test().await.unwrap();
        let club = db.default_club_id().await.unwrap();
        let hash = crate::club::auth::hash_password("password12").unwrap();
        let pid = crate::club::auth::new_id();
        sqlx::query(
            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
             VALUES ($1, $2, 'Import', 'import@test', '', 'approved', 'pilot', $3, $4)",
        )
        .bind(&pid)
        .bind(&club)
        .bind(&hash)
        .bind(now_ms())
        .execute(&db.pool)
        .await
        .unwrap();
        let token = "0123456789abcdef0123456789abcdef";
        let raw = format!("{token}=import@test\n");
        let n1 = db.import_device_tokens(&raw).await.unwrap();
        assert_eq!(n1, 1);
        let n2 = db.import_device_tokens(&raw).await.unwrap();
        assert_eq!(n2, 0);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE pilot_id = $1")
            .bind(&pid)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
