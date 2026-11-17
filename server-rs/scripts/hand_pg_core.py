#!/usr/bin/env python3
"""Inject hand-converted core ClubDb methods, then run full_pg_convert transforms."""
from __future__ import annotations

import re
import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src/club/db_sqlite_merged.rs"
OUT = ROOT / "src/club/db.rs"

CORE = r'''
    async fn bootstrap(&self) -> Result<(), DbError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM clubs")
            .fetch_one(&self.pool)
            .await?;
        if count == 0 {
            let now = now_ms();
            let club_id = new_id();
            let slug = std::env::var("FLIGHT_CLUB_SLUG").unwrap_or_else(|_| "default".to_string());
            sqlx::query(
                "INSERT INTO clubs (id, name, slug, created_at, checkin_window_hours, latitude, longitude)
                 VALUES ($1, $2, $3, $4, 12, -33.4489, -70.6693)",
            )
            .bind(club_id)
            .bind(std::env::var("FLIGHT_CLUB_NAME").unwrap_or_else(|_| "Flight Club".to_string()))
            .bind(slug)
            .bind(now)
            .execute(&self.pool)
            .await?;
        }
        if let (Ok(email), Ok(password)) = (
            std::env::var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_EMAIL"),
            std::env::var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_PASSWORD"),
        ) {
            if !email.is_empty() && !password.is_empty() {
                let club_id: String = sqlx::query_scalar(
                    "SELECT id FROM clubs ORDER BY created_at ASC, id ASC LIMIT 1",
                )
                .fetch_one(&self.pool)
                .await?;
                let exists: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM pilots WHERE club_id = $1 AND email = $2",
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
                    .bind(&club_id)
                    .bind("Admin")
                    .bind(&email)
                    .bind(&hash)
                    .bind(now_ms())
                    .execute(&self.pool)
                    .await?;
                }
            }
        }
        Ok(())
    }

    pub async fn default_club_id(&self) -> Result<String, DbError> {
        sqlx::query_scalar("SELECT id FROM clubs ORDER BY created_at ASC, id ASC LIMIT 1")
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
        let now = now_ms();
        let member_id = new_id();
        let r = sqlx::query(
            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
             VALUES ($1, $2, $3, $4, $5, 'pending', 'pilot', $6, $7)",
        )
        .bind(&id)
        .bind(club_id)
        .bind(name)
        .bind(email)
        .bind(license_no)
        .bind(&hash)
        .bind(now)
        .execute(&self.pool)
        .await;
        match r {
            Ok(_) => {
                sqlx::query(
                    "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
                     VALUES ($1, $2, $3, 'pilot', 'pending', $4)",
                )
                .bind(member_id)
                .bind(&id)
                .bind(club_id)
                .bind(now)
                .execute(&self.pool)
                .await
                .map_err(RegisterError::Db)?;
                Ok(id)
            }
            Err(e) => {
                if let Some(db_err) = e.as_database_error() {
                    if db_err.code() == Some(std::borrow::Cow::Borrowed("23505")) {
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
        .map_err(|_| LoginError::Invalid)?;
        let Some(row) = row else {
            return Err(LoginError::Invalid);
        };
        let id: String = row.try_get(0).map_err(|_| LoginError::Invalid)?;
        let club_id: String = row.try_get(1).map_err(|_| LoginError::Invalid)?;
        let name: String = row.try_get(2).map_err(|_| LoginError::Invalid)?;
        let email: String = row.try_get(3).map_err(|_| LoginError::Invalid)?;
        let license_no: String = row.try_get(4).map_err(|_| LoginError::Invalid)?;
        let status: String = row.try_get(5).map_err(|_| LoginError::Invalid)?;
        let role: String = row.try_get(6).map_err(|_| LoginError::Invalid)?;
        let hash: String = row.try_get(7).map_err(|_| LoginError::Invalid)?;
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
        let row = sqlx::query(
            "SELECT p.id, p.club_id, p.name, p.email, p.license_no, p.status, p.role
             FROM pilot_sessions s
             JOIN pilots p ON p.id = s.pilot_id
             WHERE s.id = $1 AND s.expires_at > $2",
        )
        .bind(session_id)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        Some(PilotRow {
            id: row.try_get(0).ok()?,
            club_id: row.try_get(1).ok()?,
            name: row.try_get(2).ok()?,
            email: row.try_get(3).ok()?,
            license_no: row.try_get(4).ok()?,
            status: row.try_get(5).ok()?,
            role: row.try_get(6).ok()?,
        })
    }

    CORE_MARKER_REMOVE_ME
'''


def strip_core_methods(text: str) -> str:
    """Remove sqlite bootstrap through pilot_from_session."""
    start = text.index("    fn bootstrap(&self)")
    end = text.index("    pub fn viewer_from_session")
    return text[:start] + "    CORE_INJECT\n\n" + text[end:]


def strip_sqlite_runtime(text: str) -> str:
    text = re.sub(r"const MIGRATION_\d+:.*\n", "", text)
    for pat in (
        r"    pub fn open\(data_dir: &Path\)[\s\S]*?\n    \}\n\n",
        r"    #\[cfg\(test\)\]\n    pub fn open_in_memory\(\)[\s\S]*?\n    \}\n\n",
        r"    pub\(crate\) fn with_conn<F, T>\([\s\S]*?\n    \}\n\n",
        r"    pub\(crate\) fn with_conn_any<T, E, F>\([\s\S]*?\n    \}\n\n",
        r"    fn migrate\(&self\)[\s\S]*?\n    \}\n\n",
    ):
        text = re.sub(pat, "", text, count=1)
    return text


def main() -> None:
    spec = importlib.util.spec_from_file_location("fpc", ROOT / "scripts/full_pg_convert.py")
    fpc = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fpc)
    fpc.HEADER = fpc.HEADER.replace("    pool: PgPool,", "    pub(crate) pool: PgPool,")

    raw = SRC.read_text()
    raw = strip_core_methods(raw)
    ts = raw.index("pub struct PilotRow")
    impl_i = raw.index("impl ClubDb {")
    types = raw[ts:impl_i].replace("rusqlite::Error", "DbError")
    impl_rest = raw[impl_i:]
    tests_i = impl_rest.find("\npub fn now_ms")
    impl_only = impl_rest[:tests_i] if tests_i > 0 else impl_rest
    tail = impl_rest[tests_i:] if tests_i > 0 else ""

    impl_only = impl_only.replace("CORE_INJECT", CORE.strip())
    impl_only = strip_sqlite_runtime(impl_only)

    impl_only = fpc.sql_pg_strings(impl_only)
    impl_only = fpc.unwrap_with_conn(impl_only)
    impl_only = fpc.convert_executes(impl_only)
    impl_only = fpc.convert_query_row_scalar(impl_only)
    impl_only = fpc.convert_query_row_tuple(impl_only)
    impl_only = fpc.convert_query_row_optional(impl_only)
    impl_only = fpc.convert_prepare_query_map(impl_only)
    impl_only = fpc.patch_common(impl_only)
    impl_only = fpc.patch_append_fixes(impl_only)
    impl_only = re.sub(r"\n    pub fn ", r"\n    pub async fn ", impl_only)
    for fn in ("checkout_ok", "insert_checkin_row", "checkin_replay", "latest_device_fixes"):
        impl_only = re.sub(rf"\n    fn {fn}\(", rf"\n    async fn {fn}(", impl_only)
    impl_only = impl_only.replace("conn: &Connection,", "pool: &PgPool,")
    impl_only = fpc.patch_awaits(impl_only)
    impl_only = impl_only.replace("rusqlite::Error", "DbError")
    impl_only = impl_only.replace("CORE_MARKER_REMOVE_ME", "")
    impl_only = impl_only.replace("impl ClubDb {", "impl ClubDb {" + fpc.CONNECT, 1)
    tail = fpc.convert_tests(tail.replace("rusqlite::Error", "sqlx::Error"))
    out = fpc.HEADER + types + impl_only + tail
    OUT.write_text(out)
    print("conn", out.count("conn."), "params", out.count("params!"), "lines", len(out.splitlines()))


if __name__ == "__main__":
    main()