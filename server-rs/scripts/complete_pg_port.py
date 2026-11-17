#!/usr/bin/env python3
"""Full ClubDb sqlite -> sqlx port using full_pg_convert transforms + hand core."""
from __future__ import annotations

import importlib.util
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src/club/db_sqlite_merged.rs"
ACCESS = ROOT / "src/club/db_access.rs"
OUT_DB = ROOT / "src/club/db.rs"
OUT_ACCESS = ROOT / "src/club/db_access.rs"

spec = importlib.util.spec_from_file_location("fpc", ROOT / "scripts/full_pg_convert.py")
fpc = importlib.util.module_from_spec(spec)
assert spec.loader
spec.loader.exec_module(fpc)

HEADER = fpc.HEADER.replace("    pool: PgPool,", "    pub(crate) pool: PgPool,")

DEVICE_TYPES = """

pub const MAX_DEVICES_PER_PILOT: usize = 10;
pub const MIN_INSTALL_ID_LEN: usize = 8;
pub const MAX_INSTALL_ID_LEN: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceRegisterError {
    InvalidInstallId,
    CapExceeded,
    Db,
}

"""

SHARE_TYPES = """

pub enum ShareCreateError {
    Db(DbError),
    NotFound,
    NotOwner,
}

pub enum ShareRevokeError {
    Db(DbError),
    NotFound,
    NotOwner,
}

"""

HAND_CORE = r"""
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
"""


def strip_plumbing(text: str) -> str:
    for pat in (
        r"const MIGRATION_\d+:.*\n",
        r"    pub fn open\(data_dir: &Path\)[\s\S]*?\n    \}\n\n",
        r"    #\[cfg\(test\)\]\n    pub fn open_in_memory\(\)[\s\S]*?\n    \}\n\n",
        r"    pub\(crate\) fn with_conn<F, T>\([\s\S]*?\n    \}\n\n",
        r"    pub\(crate\) fn with_conn_any<T, E, F>\([\s\S]*?\n    \}\n\n",
        r"    fn migrate\(&self\)[\s\S]*?\n    \}\n\n",
    ):
        text = re.sub(pat, "", text, count=1)
    text = re.sub(
        r"    fn bootstrap\(&self\) -> Result<(), rusqlite::Error> \{[\s\S]*?\n    \}\n\n",
        HAND_CORE + "\n",
        text,
        count=1,
    )
    return text


def merge_access_impl(sqlite_core: str) -> str:
    if not ACCESS.exists():
        return sqlite_core
    acc = ACCESS.read_text()
    extra = []
    for m in re.finditer(r"impl ClubDb \{([\s\S]*?)\n\}", acc):
        extra.append(m.group(1))
    if not extra:
        return sqlite_core
    idx = sqlite_core.rfind("\n}\n\npub fn now_ms")
    if idx < 0:
        idx = sqlite_core.rfind("\n}\n")
    return sqlite_core[:idx] + "\n" + "\n".join(extra) + sqlite_core[idx:]


def main() -> None:
    # Fresh sqlite access from origin if rusqlite present in db.rs access file
    raw = SRC.read_text()
    raw = merge_access_impl(raw)
    ts = raw.index("pub struct PilotRow")
    impl_i = raw.index("impl ClubDb {")
    pre_impl = raw[:impl_i]
    types = raw[ts:impl_i].replace("rusqlite::Error", "DbError")
    if "DeviceRegisterError" not in types:
        types += DEVICE_TYPES
    if "ShareCreateError" not in types:
        types += SHARE_TYPES
    impl_rest = raw[impl_i:]
    tests_i = impl_rest.find("\npub fn now_ms")
    impl_only = impl_rest[:tests_i] if tests_i > 0 else impl_rest
    tail = impl_rest[tests_i:] if tests_i > 0 else ""

    impl_only = strip_plumbing(impl_only)
    impl_only = fpc.transform_impl(impl_only)
    impl_only = impl_only.replace("impl ClubDb {", "impl ClubDb {" + fpc.CONNECT, 1)
    # data_dir for DEM on postgres
    if "fn data_dir" not in impl_only:
        impl_only = impl_only.replace(
            "impl ClubDb {",
            'impl ClubDb {\n    pub(crate) fn data_dir(&self) -> &std::path::Path {\n        static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();\n        DIR.get_or_init(|| {\n            std::env::var("FLIGHT_DATA")\n                .map(std::path::PathBuf::from)\n                .unwrap_or_else(|_| std::path::PathBuf::from("data"))\n        })\n    }\n',
            1,
        )
    else:
        impl_only = re.sub(
            r"pub\(crate\) fn data_dir\(&self\)[\s\S]*?\n    \}\n",
            """pub(crate) fn data_dir(&self) -> &std::path::Path {
        static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        DIR.get_or_init(|| {
            std::env::var("FLIGHT_DATA")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("data"))
        })
    }
""",
            impl_only,
            count=1,
        )

    tail = fpc.convert_tests(tail.replace("rusqlite::Error", "sqlx::Error"))
    out = HEADER + types + impl_only + tail

    auth_core = (ROOT / "scripts/auth_core.rs.snippet").read_text()
    out = re.sub(
        r"async fn bootstrap\(&self\)[\s\S]*?pub async fn viewer_from_session",
        auth_core.strip() + "\n\n    pub async fn viewer_from_session",
        out,
        count=1,
    )

    # cleanup leftovers: second pass converters
    for _ in range(3):
        before = out.count("conn.")
        out = fpc.unwrap_with_conn_brace(out)
        out = fpc.unwrap_with_conn_paren(out)
        out = fpc.convert_executes(out)
        out = fpc.convert_query_row_scalar(out)
        out = fpc.convert_query_row_tuple(out)
        out = fpc.convert_query_row_optional(out)
        out = fpc.convert_prepare_query_map(out)
        if out.count("conn.") == before:
            break

    OUT_DB.write_text(out)
    # db_access empty — methods merged into db.rs
    OUT_ACCESS.write_text(
        ACCESS_HEADER
        + "\n// access methods live in db.rs\n"
        if (ACCESS_HEADER := """use crate::club::access_request::{
    normalize_tail, AccessRequestError, AccessRequestView, AdminAccessRequestView,
};
use crate::club::auth::new_id;
use crate::club::db::{now_ms, CheckoutError, ClubDb, DbError};
use serde::Serialize;
use sqlx::{PgPool, Row};
use std::borrow::Cow;

""")
        else ""
    )
    leftovers = out.count("conn.") + out.count("params!") + out.count("with_conn")
    print("wrote", OUT_DB, "lines", len(out.splitlines()), "leftovers", leftovers)
    out = re.sub(r"\)\.bind\((\w+):", r", \1:", out)
    out = out.replace("Result<(), DbError>", "Result<(), DbError>")
    out = out.replace("Result<usize).bind(DbError>", "Result<usize, DbError>")
    out = out.replace("Result<()).bind(DbError>", "Result<(), DbError>")
    OUT_DB.write_text(out)
    if leftovers:
        print("WARN still has sqlite markers")


if __name__ == "__main__":
    main()