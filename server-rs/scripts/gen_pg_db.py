#!/usr/bin/env python3
"""Generate server-rs/src/club/db.rs (Postgres/sqlx) from db_sqlite_reference.rs."""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src/club/db_sqlite_reference.rs"
OUT = ROOT / "src/club/db.rs"

HEADER = r'''use crate::club::{
    auth::{self, digest_token, new_device_token, new_id, SESSION_TTL_SEC},
    checkin::{self, CheckinTokenError},
    database::{self, DbConnectError},
    map::{dedupe_by_tail, filter_admin_tails, DeviceFix, TailLivePosition},
    visibility::{
        can_view_flight, includes_in_club_map, includes_in_public_map, show_pilot_name,
        FlightVisibility, PilotRole, PilotStatus, Viewer,
    },
};
use crate::live::LiveFix;
use serde::Serialize;
use sqlx::{PgPool, Row};
use std::borrow::Cow;

pub type DbError = sqlx::Error;

#[derive(Clone)]
pub struct ClubDb {
    pool: PgPool,
}

'''


def sql_pg(s: str) -> str:
    return re.sub(r"\?(\d+)", r"$\1", s)


def rewrite_string_literals(text: str) -> str:
    def repl(m: re.Match[str]) -> str:
        inner = m.group(1)
        if any(
            kw in inner
            for kw in (
                "SELECT",
                "INSERT",
                "UPDATE",
                "DELETE",
                "FROM",
                "TRUNCATE",
            )
        ):
            return '"' + sql_pg(inner) + '"'
        return m.group(0)

    return re.sub(r'"((?:[^"\\]|\\.)*)"', repl, text)


def strip_sqlite_plumbing(body: str) -> str:
    patterns = [
        r"const MIGRATION_\d+:.*\n",
        r"#\[derive\(Clone\)\]\npub struct ClubDb \{[^}]+\}\n\n",
        r"    pub fn open\(data_dir: &Path\)[\s\S]*?Ok\(db\)\n    \}\n\n",
        r"    #\[cfg\(test\)\]\n    pub fn open_in_memory\(\)[\s\S]*?Ok\(db\)\n    \}\n\n",
        r"    fn with_conn<F, T>\([\s\S]*?\n    \}\n\n",
        r"    fn migrate\(&self\)[\s\S]*?\n        \}\)\n    \}\n\n",
    ]
    for p in patterns:
        body = re.sub(p, "", body, count=1)
    return body


def unwrap_with_conn(body: str) -> str:
    needle = "self.with_conn(|conn| {"
    while needle in body:
        start = body.index(needle)
        brace = start + len(needle) - 1
        depth = 0
        end = None
        for i in range(brace, len(body)):
            c = body[i]
            if c == "{":
                depth += 1
            elif c == "}":
                depth -= 1
                if depth == 0:
                    end = i
                    break
        if end is None:
            raise RuntimeError("unbalanced with_conn")
        tail = end + 1
        if tail < len(body) and body[tail] == ")":
            tail += 1
        inner = body[brace + 1 : end]
        body = body[:start] + "{\n" + inner + body[tail:]
    body = body.replace("Self::checkout_ok(conn,", "Self::checkout_ok(&self.pool,")
    body = body.replace("db.with_conn(|conn| {", "{\n        // test SQL:\n        let pool = &db.pool;\n        ")
    # fix test with_conn manually later
    return body


def params_to_bind(params: str) -> str:
    params = params.strip()
    if not params.startswith("params!["):
        return ""
    inner = params[len("params![") : -1].strip()
    if not inner:
        return ""
    parts: list[str] = []
    depth = 0
    buf: list[str] = []
    for ch in inner:
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        if ch == "," and depth == 0:
            p = "".join(buf).strip()
            if p:
                parts.append(p)
            buf = []
            continue
        buf.append(ch)
    tail = "".join(buf).strip()
    if tail:
        parts.append(tail)
    return "".join(f".bind({p})" for p in parts)


def convert_conn_execute(body: str) -> str:
    pat = re.compile(
        r"conn\.execute\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*,\s*(params!\[[\s\S]*?\])\s*\)\?",
        re.MULTILINE,
    )

    def repl(m: re.Match[str]) -> str:
        sql = m.group(1).strip()
        binds = params_to_bind(m.group(2))
        if "INSERT INTO fixes" in sql and "ON CONFLICT" not in sql:
            sql = sql.replace(
                '"INSERT INTO fixes',
                '"INSERT INTO fixes',
            )
        return f"sqlx::query({sql}){binds}.execute(pool).await?"

    return pat.sub(repl, body)


def convert_body(body: str) -> str:
    body = body.replace("rusqlite::Error", "DbError")
    body = rewrite_string_literals(body)
    body = unwrap_with_conn(body)
    body = convert_conn_execute(body)
    # pool variable name in blocks
    body = body.replace("sqlx::query", "sqlx::query")
    body = re.sub(
        r"(\n)(\s*)sqlx::query",
        lambda m: f"{m.group(1)}{m.group(2)}sqlx::query",
        body,
    )
    # After unwrap, insert `let pool = &self.pool;` at start of each async method block - too hard
    body = body.replace(".execute(pool).await?", ".execute(&self.pool).await?")

    body = body.replace("conn.query_row(", "/*QROW*/conn.query_row(")
    return body


def main() -> None:
    raw = SRC.read_text()
    ts = raw.index("pub struct PilotRow")
    impl_start = raw.index("impl ClubDb {")
    types = raw[ts:impl_start].replace("rusqlite::Error", "DbError")
    body = raw[impl_start:]
    body = strip_sqlite_plumbing(body)
    body = convert_body(body)

    open_impl = '''
    pub async fn connect() -> Result<Self, DbConnectError> {
        let pool = database::connect_pool().await?;
        database::run_migrations(&pool).await?;
        let db = Self { pool: pool.clone() };
        db.bootstrap().await?;
        Ok(db)
    }

    #[cfg(test)]
    pub async fn open_test() -> Result<Self, DbConnectError> {
        let pool = database::open_test_pool().await?;
        let db = Self { pool: pool.clone() };
        db.truncate_all().await.map_err(DbConnectError::from)?;
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

    body = re.sub(r"\n    pub fn ", r"\n    pub async fn ", body)
    body = re.sub(r"\n    fn checkout_ok", r"\n    async fn checkout_ok", body)
    body = re.sub(r"\n    fn insert_checkin_row", r"\n    async fn insert_checkin_row", body)
    body = re.sub(r"\n    fn checkin_replay", r"\n    async fn checkin_replay", body)
    body = re.sub(r"\n    fn latest_device_fixes", r"\n    async fn latest_device_fixes", body)
    body = re.sub(r"\n    fn bootstrap\(", r"\n    async fn bootstrap(", body)

    body = body.replace("conn: &Connection,", "pool: &PgPool,")
    body = body.replace("i64::from(show_pilot_name_public)", "show_pilot_name_public")
    body = body.replace("i64::from(flag)", "flag")
    body = body.replace("i64::from(active)", "active")
    body = body.replace("r.get::<_, i64>(8)? != 0", "r.try_get::<_, bool>(8)?")
    body = body.replace("r.get::<_, i64>(6)? != 0", "r.try_get::<_, bool>(6)?")
    body = body.replace("r.get::<_, i64>(3)? != 0", "r.try_get::<_, bool>(3)?")
    body = body.replace("(type_id, active): (String, i64)", "(type_id, active): (String, bool)")
    body = body.replace("if active == 0", "if !active")
    body = body.replace(
        """if let rusqlite::Error::SqliteFailure(err, _) = &e {
                    if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE {
                        return Err(RegisterError::EmailExists);
                    }
                }""",
        """if let Some(db_err) = e.as_database_error() {
                    if db_err.code() == Some(Cow::Borrowed("23505")) {
                        return Err(RegisterError::EmailExists);
                    }
                }""",
    )
    body = body.replace(
        """return Err(rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                        Some("unknown tail".into()),
                    ));""",
        "return Err(sqlx::Error::RowNotFound);",
    )
    body = body.replace(
        "INSERT OR IGNORE INTO fixes",
        "INSERT INTO fixes",
    )

    body = body.replace("ClubDb::open_in_memory().unwrap()", "ClubDb::open_test().await.unwrap()")
    body = re.sub(r"#\[test\]\n    fn ", "#[tokio::test]\n    async fn ", body)

    out = HEADER + types + body
  if "/*QROW*/" in out:
        print("WARNING: leftover query_row markers — manual fix required", file=sys.stderr)
    OUT.write_text(out)
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()