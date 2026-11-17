#!/usr/bin/env python3
"""Mechanical SQLite/rusqlite → sqlx/Postgres conversion for club/db.rs."""
import re
import sys
from pathlib import Path

SRC = Path(__file__).resolve().parents[1] / "src/club/db.rs"
OUT = Path(__file__).resolve().parents[1] / "src/club/db.rs.new"

HEADER = r'''use crate::club::{
    auth::{self, digest_token, new_device_token, new_id, SESSION_TTL_SEC},
    checkin::{self, CheckinTokenError},
    database,
    map::{dedupe_by_tail, filter_admin_tails, DeviceFix, TailLivePosition},
    visibility::{
        can_view_flight, includes_in_club_map, includes_in_public_map, show_pilot_name,
        FlightVisibility, PilotRole, PilotStatus, Viewer,
    },
};
use crate::live::LiveFix;
use serde::Serialize;
use sqlx::{postgres::PgRow, PgPool, Row};
use std::borrow::Cow;

pub type DbError = sqlx::Error;

#[derive(Clone)]
pub struct ClubDb {
    pool: PgPool,
}

'''

def pg_sql(sql: str) -> str:
    sql = re.sub(r"\?(\d+)", r"$\1", sql)
    sql = sql.replace("INSERT OR IGNORE INTO fixes", "INSERT INTO fixes")
    return sql

def main() -> None:
    raw = SRC.read_text()
    # Strip old header through FlightRow struct end — we rebuild header
    start = raw.find("impl ClubDb")
    if start < 0:
        sys.exit("impl ClubDb not found")
    body = raw[start:]
    body = body.replace("rusqlite::Error", "DbError")
    body = body.replace("rusqlite::Error", "DbError")
    for n in range(1, 30):
        body = body.replace(f"?{n}", f"${n}")

    # Struct and open paths
    body = re.sub(
        r"pub struct ClubDb \{[^}]+\}",
        "",
        body,
        count=1,
    )
    body = body.replace(
        """    pub fn open(data_dir: &Path) -> Result<Self, DbError> {
        let path = data_dir.join("club.sqlite");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let db = Self {
            path,
            conn: Arc::new(Mutex::new(conn)),
        };
        db.migrate()?;
        db.bootstrap()?;
        Ok(db)
    }""",
        "",
    )
    body = body.replace(
        """    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let db = Self {
            path: PathBuf::from(":memory:"),
            conn: Arc::new(Mutex::new(conn)),
        };
        db.migrate()?;
        db.bootstrap()?;
        Ok(db)
    }""",
        "",
    )
    body = re.sub(
        r"    fn with_conn<F, T>\([^)]+\)[^{]+\{[^}]+\}\n",
        "",
        body,
        count=1,
        flags=re.DOTALL,
    )
    body = re.sub(
        r"    fn migrate\(&self\)[^}]+\}\n    \}\n",
        "",
        body,
        count=1,
        flags=re.DOTALL,
    )
    body = re.sub(
        r"const MIGRATION_\d+.*?\n",
        "",
        body,
    )

    # Remove path field references
    body = body.replace("#[allow(dead_code)]\n    path: PathBuf,\n    conn: Arc<Mutex<Connection>>,\n", "")

    # Add new open methods after impl ClubDb {
    open_impl = '''
    pub async fn connect() -> Result<Self, database::DbConnectError> {
        let pool = database::connect_pool().await?;
        database::run_migrations(&pool).await?;
        let db = Self { pool };
        db.bootstrap().await?;
        Ok(db)
    }

  pub(crate) async fn connect_pool(pool: PgPool) -> Result<Self, DbError> {
        let db = Self { pool };
        db.bootstrap().await?;
        Ok(db)
    }

    #[cfg(test)]
    pub async fn open_test() -> Result<Self, database::DbConnectError> {
        let pool = database::open_test_pool().await?;
        let db = Self { pool };
        db.truncate_all().await?;
        db.bootstrap().await?;
        Ok(db)
    }

    async fn truncate_all(&self) -> Result<(), DbError> {
        sqlx::query(
            "TRUNCATE TABLE checkins, device_pair_tokens, flight_devices, fixes, flights, \\
             devices, checkouts, pilot_sessions, admin_map_prefs, display_sessions, \\
             pilots, aircraft, aircraft_types, clubs RESTART IDENTITY CASCADE",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

'''
    body = body.replace("impl ClubDb {", "impl ClubDb {" + open_impl, 1)

    body = body.replace("    fn bootstrap(&self)", "    async fn bootstrap(&self)")
    body = body.replace(
        "        self.with_conn(|conn| {",
        "        {",
    )
    # Fix bootstrap - replace with_conn block manually later

    # pub fn -> pub async fn for impl methods
    body = re.sub(
        r"(\n    )pub fn ([a-z_]+)\(",
        r"\1pub async fn \2(",
        body,
    )
    body = re.sub(
        r"(\n    )fn ([a-z_]+)\(",
        r"\1async fn \2(",
        body,
    )

    # RegisterError / other error enums
    preamble = raw[:start]
    # keep types from preamble after removing rusqlite header
    types_start = raw.find("#[derive(Clone)]\npub struct PilotRow")
    types = raw[types_start:start]
    types = types.replace("use rusqlite", "// removed rusqlite")

    out = HEADER + types + body
    OUT.write_text(out)
    print(f"wrote {OUT} ({len(out.splitlines())} lines) — manual pass required")

if __name__ == "__main__":
    main()