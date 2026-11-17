#!/usr/bin/env python3
"""Convert club/db SQLite → sqlx Postgres."""
from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src/club/db_sqlite_reference.rs"
OUT = ROOT / "src/club/db.rs"

HEADER = '''use crate::club::{
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

'''


def pg_params(sql: str) -> str:
    return re.sub(r"\?(\d+)", r"$\1", sql)


def convert_params_to_bind(params: str) -> str:
    if not params.strip().startswith("params!["):
        return ""
    inner = params.strip()[len("params![") : -1]
    items: list[str] = []
    depth = 0
    buf: list[str] = []
    for ch in inner:
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        if ch == "," and depth == 0:
            part = "".join(buf).strip()
            if part:
                items.append(part)
            buf = []
            continue
        buf.append(ch)
    tail = "".join(buf).strip()
    if tail:
        items.append(tail)
    return "".join(f".bind({x})" for x in items)


def rewrite_sql_strings(text: str) -> str:
    def repl(m: re.Match[str]) -> str:
        return '"' + pg_params(m.group(1)) + '"'

    return re.sub(r'"((?:[^"\\]|\\.)*)"', repl, text)


def unwrap_with_conn(text: str) -> str:
    needle = "self.with_conn(|conn| {"
    while True:
        start = text.find(needle)
        if start == -1:
            break
        brace = start + len(needle) - 1
        depth = 0
        end = None
        for i in range(brace, len(text)):
            c = text[i]
            if c == "{":
                depth += 1
            elif c == "}":
                depth -= 1
                if depth == 0:
                    end = i
                    break
        if end is None:
            break
        tail = end + 1
        if tail < len(text) and text[tail] == ")":
            tail += 1
        inner = text[brace + 1 : end]
        text = text[:start] + "{\n" + inner + "\n" + text[tail:]
    text = text.replace("Self::checkout_ok(conn,", "Self::checkout_ok(&self.pool,")
    return text


def convert_executes(text: str) -> str:
    pattern = re.compile(
        r"conn\.execute\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*,\s*(params!\[[\s\S]*?\])\s*\)\?",
        re.MULTILINE,
    )

    def repl(m: re.Match[str]) -> str:
        sql = m.group(1).strip()
        binds = convert_params_to_bind(m.group(2))
        return f"sqlx::query({sql}){binds}.execute(&self.pool).await?"

    return pattern.sub(repl, text)


def main() -> None:
    raw = SRC.read_text()
    types_start = raw.index("pub struct PilotRow")
    impl_start = raw.index("impl ClubDb {")
    types = raw[types_start:impl_start]
    types = types.replace("rusqlite::Error", "DbError")
    body = raw[impl_start:]

    for pat in (
        r"const MIGRATION_\d+:.*\n",
        r"#\[derive\(Clone\)\]\npub struct ClubDb \{[^}]+\}\n\n",
        r"    pub fn open\(data_dir: &Path\)[\s\S]*?Ok\(db\)\n    \}\n\n",
        r"    #\[cfg\(test\)\]\n    pub fn open_in_memory\(\)[\s\S]*?Ok\(db\)\n    \}\n\n",
        r"    fn with_conn<F, T>\([\s\S]*?\n    \}\n\n",
        r"    fn migrate\(&self\)[\s\S]*?\n        \}\)\n    \}\n\n",
    ):
        body = re.sub(pat, "", body, count=1)

    body = rewrite_sql_strings(body)
    body = body.replace("rusqlite::Error", "DbError")
    body = unwrap_with_conn(body)
    body = convert_executes(body)

    open_impl = '''
    pub async fn connect() -> Result<Self, DbConnectError> {
        let pool = database::connect_pool().await?;
        database::run_migrations(&pool).await?;
        let db = Self { pool };
        db.bootstrap().await?;
        Ok(db)
    }

    #[cfg(test)]
    pub async fn open_test() -> Result<Self, DbConnectError> {
        let pool = database::open_test_pool().await?;
        let db = Self { pool };
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
    struct = "#[derive(Clone)]\npub struct ClubDb {\n    pool: PgPool,\n}\n\n"
    body = struct + body.replace("impl ClubDb {", "impl ClubDb {" + open_impl, 1)

    body = body.replace("    fn bootstrap(&self)", "    async fn bootstrap(&self)")
    body = re.sub(r"(\n    )pub fn ", r"\1pub async fn ", body)
    body = re.sub(r"(\n    )async fn checkout_ok", r"\1async fn checkout_ok", body)
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

    body = body.replace("ClubDb::open_in_memory().unwrap()", "ClubDb::open_test().await.unwrap()")
    body = re.sub(r"#\[test\]\n    fn ", "#[tokio::test]\n    async fn ", body)

    out = HEADER + types + body
    OUT.write_text(out)
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()