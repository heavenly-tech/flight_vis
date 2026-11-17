#!/usr/bin/env python3
from pathlib import Path

p = Path(__file__).resolve().parents[1] / "src/club/db_pg.rs"
text = p.read_text()
start = text.index("    async fn bootstrap(")
end = text.index("    pub async fn viewer_from_session")
auth = '''    async fn bootstrap(&self) -> Result<(), DbError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM clubs")
            .fetch_one(&self.pool)
            .await?;
        if count == 0 {
            let now = now_ms();
            let club_id = new_id();
            sqlx::query("INSERT INTO clubs (id, name, created_at) VALUES ($1, $2, $3)")
                .bind(club_id)
                .bind(
                    std::env::var("FLIGHT_CLUB_NAME")
                        .unwrap_or_else(|_| "Flight Club".to_string()),
                )
                .bind(now)
                .execute(&self.pool)
                .await?;
        }
        if let (Ok(email), Ok(password)) = (
            std::env::var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_EMAIL"),
            std::env::var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_PASSWORD"),
        ) {
            if !email.is_empty() && !password.is_empty() {
                let club_id: String = sqlx::query_scalar("SELECT id FROM clubs LIMIT 1")
                    .fetch_one(&self.pool)
                    .await?;
                let exists: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*)::bigint FROM pilots WHERE club_id = $1 AND email = $2",
                )
                .bind(&club_id)
                .bind(&email)
                .fetch_one(&self.pool)
                .await?;
                if exists == 0 {
                    let hash = auth::hash_password(&password)
                        .map_err(|_| DbError::InvalidParameterName("hash".into()))?;
                    sqlx::query(
                        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
                         VALUES ($1, $2, $3, $4, '', 'approved', 'admin', $5, $6)",
                    )
                    .bind(new_id())
                    .bind(club_id)
                    .bind("Admin")
                    .bind(email)
                    .bind(hash)
                    .bind(now_ms())
                    .execute(&self.pool)
                    .await?;
                }
            }
        }
        Ok(())
    }

    pub async fn default_club_id(&self) -> Result<String, DbError> {
        sqlx::query_scalar("SELECT id FROM clubs LIMIT 1")
            .fetch_one(&self.pool)
            .await
    }

    pub async fn register(
        &self,
        club_id: &str,
        name: &str,
        email: &str,
        license_no: &str,
        password: &str,
    ) -> Result<String, RegisterError> {
        if password.len() < 8 {
            return Err(RegisterError::WeakPassword);
        }
        let hash = auth::hash_password(password).map_err(|_| RegisterError::Hash)?;
        let id = new_id();
        match sqlx::query(
            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
             VALUES ($1, $2, $3, $4, $5, 'pending', 'pilot', $6, $7)",
        )
        .bind(&id)
        .bind(club_id)
        .bind(name)
        .bind(email)
        .bind(license_no)
        .bind(hash)
        .bind(now_ms())
        .execute(&self.pool)
        .await
        {
            Ok(_) => Ok(id),
            Err(e) => {
                if let Some(db_err) = e.as_database_error() {
                    if db_err.code() == Some(Cow::Borrowed("23505")) {
                        return Err(RegisterError::EmailExists);
                    }
                }
                Err(RegisterError::Db(e))
            }
        }
    }

    pub async fn login(
        &self,
        club_id: &str,
        email: &str,
        password: &str,
        now: i64,
    ) -> Result<(String, PilotRow), LoginError> {
        let row = sqlx::query(
            "SELECT id, club_id, name, email, license_no, status, role, password_hash
             FROM pilots WHERE club_id = $1 AND email = $2",
        )
        .bind(club_id)
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| LoginError::Invalid)?
        .map(|r| -> Result<_, DbError> {
            Ok((
                r.try_get(0)?,
                r.try_get(1)?,
                r.try_get(2)?,
                r.try_get(3)?,
                r.try_get(4)?,
                r.try_get(5)?,
                r.try_get(6)?,
                r.try_get(7)?,
            ))
        })
        .transpose()
        .map_err(|_| LoginError::Invalid)?;
        let Some((id, club_id, name, email, license_no, status, role, hash)) = row else {
            return Err(LoginError::Invalid);
        };
        if !auth::verify_password(password, &hash) {
            return Err(LoginError::Invalid);
        }
        let session_id = auth::new_session_id();
        let exp = now + SESSION_TTL_SEC * 1000;
        sqlx::query("INSERT INTO pilot_sessions (id, pilot_id, expires_at) VALUES ($1, $2, $3)")
            .bind(&session_id)
            .bind(&id)
            .bind(exp)
            .execute(&self.pool)
            .await
            .map_err(|_| LoginError::Invalid)?;
        Ok((
            session_id,
            PilotRow {
                id,
                club_id,
                name,
                email,
                license_no,
                status,
                role,
            },
        ))
    }

    pub async fn logout(&self, session_id: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM pilot_sessions WHERE id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn pilot_from_session(&self, session_id: &str, now: i64) -> Option<PilotRow> {
        sqlx::query(
            "SELECT p.id, p.club_id, p.name, p.email, p.license_no, p.status, p.role
             FROM pilot_sessions s
             JOIN pilots p ON p.id = s.pilot_id
             WHERE s.id = $1 AND s.expires_at > $2",
        )
        .bind(session_id)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .map(|row| PilotRow {
            id: row.try_get(0).ok()?,
            club_id: row.try_get(1).ok()?,
            name: row.try_get(2).ok()?,
            email: row.try_get(3).ok()?,
            license_no: row.try_get(4).ok()?,
            status: row.try_get(5).ok()?,
            role: row.try_get(6).ok()?,
        })
    }

'''
p.write_text(text[:start] + auth + text[end:])
print("patched auth block")