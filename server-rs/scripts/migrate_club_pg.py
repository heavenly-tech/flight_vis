#!/usr/bin/env python3
"""Generate Postgres sqlx club/db.rs + db_access.rs from db_sqlite_merged.rs."""
from __future__ import annotations

import re
import sys
from pathlib import Path

# Reuse transforms from full_pg_convert
sys.path.insert(0, str(Path(__file__).resolve().parent))
import importlib.util

spec = importlib.util.spec_from_file_location(
    "full_pg_convert", Path(__file__).parent / "full_pg_convert.py"
)
fpc = importlib.util.module_from_spec(spec)
assert spec.loader
spec.loader.exec_module(fpc)

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src/club/db_sqlite_merged.rs"
ACCESS_SRC = ROOT / "src/club/db_access.rs"
OUT = ROOT / "src/club/db.rs"
OUT_ACCESS = ROOT / "src/club/db_access.rs"

HEADER = fpc.HEADER
CONNECT = fpc.CONNECT
ACCESS_HEADER = fpc.ACCESS_HEADER


def drop_sqlite_shell(text: str) -> tuple[str, str]:
    ts = text.index("pub struct PilotRow")
    impl_i = text.index("impl ClubDb {")
    types = text[ts:impl_i].replace("rusqlite::Error", "DbError")
    impl_rest = text[impl_i:]
    tests_i = impl_rest.find("\npub fn now_ms")
    impl_only = impl_rest if tests_i < 0 else impl_rest[:tests_i]
    tail = "" if tests_i < 0 else impl_rest[tests_i:]
    return types, impl_only, tail


def transform(impl_body: str, *, is_access: bool = False) -> str:
    impl_body = fpc.strip_sqlite_plumbing(impl_body) if not is_access else impl_body
    impl_body = fpc.sql_pg_strings(impl_body)
    impl_body = fpc.unwrap_with_conn(impl_body)
    impl_body = fpc.convert_executes(impl_body)
    impl_body = fpc.convert_query_row_scalar(impl_body)
    impl_body = fpc.convert_query_row_tuple(impl_body)
    impl_body = fpc.convert_query_row_optional(impl_body)
    impl_body = fpc.convert_prepare_query_map(impl_body)
    impl_body = fpc.patch_common(impl_body)
    impl_body = fpc.patch_append_fixes(impl_body)
    impl_body = re.sub(r"\n    pub fn ", r"\n    pub async fn ", impl_body)
    for fn in (
        "bootstrap",
        "checkout_ok",
        "insert_checkin_row",
        "checkin_replay",
        "latest_device_fixes",
    ):
        impl_body = re.sub(rf"\n    fn {fn}\(", rf"\n    async fn {fn}(", impl_body)
    if is_access:
        impl_body = impl_body.replace("rusqlite::Row<'_>", "sqlx::postgres::PgRow")
        impl_body = impl_body.replace("r.get::<_", "r.try_get::<_")
    impl_body = fpc.patch_awaits(impl_body)
    impl_body = impl_body.replace("rusqlite::Error", "DbError")
    return impl_body


def fix_register_login(body: str) -> str:
    body = re.sub(
        r"match \{\s*\n\s*sqlx::query\([^\)]+\)[^\n]+\n[^\n]+\n\s*Ok\(id\)\s*\n\s*\{",
        "match sqlx::query(",
        body,
        count=3,
    )
    body = re.sub(
        r"pub async fn logout\(&self\)\.bind",
        "pub async fn logout(&self, session_id: &str) -> Result<(), DbError> {\n        sqlx::query(\"DELETE FROM pilot_sessions WHERE id = $1\")",
        body,
        count=1,
    )
    return body


def fix_bootstrap(body: str) -> str:
    replacement = '''    async fn bootstrap(&self) -> Result<(), DbError> {
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
            .bind(
                std::env::var("FLIGHT_CLUB_NAME").unwrap_or_else(|_| "Flight Club".to_string()),
            )
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
'''
    body = re.sub(
        r"async fn bootstrap\(&self\)[\s\S]*?\n    \}\n\n    pub async fn default_club_id",
        replacement + "\n    pub async fn default_club_id",
        body,
        count=1,
    )
    body = re.sub(
        r"pub async fn default_club_id\(&self\)[\s\S]*?\n    \}\n\n    pub async fn register",
        '''    pub async fn default_club_id(&self) -> Result<String, DbError> {
        sqlx::query_scalar("SELECT id FROM clubs ORDER BY created_at ASC, id ASC LIMIT 1")
            .fetch_one(&self.pool)
            .await
    }

    pub async fn register''',
        body,
        count=1,
    )
    return body


def cleanup_conn_markers(body: str) -> str:
    body = body.replace("conn.", "/*CONN*/conn.")
    return body


def convert_tests(tail: str) -> str:
    return fpc.convert_tests(tail.replace("rusqlite::Error", "sqlx::Error"))


def main() -> None:
    # Fresh sqlite access from merged file's companion - use origin db_access merged in sqlite source
    raw = SRC.read_text()
    types, impl_only, tail = drop_sqlite_shell(raw)
    impl_only = transform(impl_only)
    impl_only = impl_only.replace("impl ClubDb {", "impl ClubDb {" + CONNECT, 1)
    impl_only = fix_bootstrap(impl_only)
    impl_only = fix_register_login(impl_only)
    tail = convert_tests(tail)
    out = HEADER + types + impl_only + tail
    OUT.write_text(out)

    access_raw = ACCESS_SRC.read_text()
    access_blocks = []
    for m in re.finditer(r"impl ClubDb \{([\s\S]*?)\n\}", access_raw):
        block = transform("impl ClubDb {" + m.group(1) + "\n}", is_access=True)
        access_blocks.append(block)
    access_out = ACCESS_HEADER + "\n".join(access_blocks)
    if "pub struct AdminPendingCounts" in access_raw:
        access_out += "\n" + access_raw[access_raw.index("pub struct AdminPendingCounts") :]
    OUT_ACCESS.write_text(access_out)

    for name, path in (("db.rs", OUT), ("db_access.rs", OUT_ACCESS)):
        t = path.read_text()
        leftovers = {
            "conn.": t.count("conn."),
            "with_conn": t.count("with_conn"),
            "params!": t.count("params!"),
        }
        print(name, "leftovers", leftovers, "lines", len(t.splitlines()))


if __name__ == "__main__":
    main()