#!/usr/bin/env python3
"""Convert db.rs from sqlite reference to sqlx (conservative line-based)."""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src/club/db_sqlite_merged.rs"
OUT = ROOT / "src/club/db.rs"

HEADER = """use crate::club::{
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

"""

CONNECT = """
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

"""


def sql_fix(s: str) -> str:
    s = re.sub(r"\?(\d+)", r"$\1", s)
    return s.replace("INSERT OR IGNORE INTO fixes", "INSERT INTO fixes")


def fix_strings(text: str) -> str:
    def repl(m):
        s = m.group(1)
        if re.search(r"\b(SELECT|INSERT|UPDATE|DELETE|FROM|WHERE|VALUES|TRUNCATE)\b", s):
            return '"' + sql_fix(s) + '"'
        return m.group(0)

    return re.sub(r'"((?:[^"\\]|\\.)*)"', repl, text)


def strip_rusqlite_header(raw: str) -> str:
    i = raw.index("pub struct PilotRow")
    impl_i = raw.index("impl ClubDb {")
    types = raw[i:impl_i].replace("rusqlite::Error", "DbError")
    body = raw[impl_i:]
    return types, body


def remove_sqlite_boot(body: str) -> str:
    start = body.index("    pub fn open(data_dir:")
    end = body.index("    fn bootstrap(&self)")
    body = body[:start] + CONNECT + "    async fn bootstrap(&self)" + body[end + len("    fn bootstrap(&self)") :]
    # remove with_conn fn
    m = re.search(r"    fn with_conn<F, T>\([\s\S]*?\n    \}\n\n", body)
    if m:
        body = body[: m.start()] + body[m.end() :]
    # remove migrate fn if still there (was inside removed block - open included migrate)
    return body


def unwrap_with_conn(body: str) -> str:
    while "self.with_conn(|conn| {" in body:
        i = body.index("self.with_conn(|conn| {")
        j = i + len("self.with_conn(|conn| {") - 1
        depth = 0
        end = None
        for k in range(j, len(body)):
            if body[k] == "{":
                depth += 1
            elif body[k] == "}":
                depth -= 1
                if depth == 0:
                    end = k
                    break
        if end is None:
            raise ValueError("unbalanced with_conn")
        t = end + 1
        if t < len(body) and body[t] == ")":
            t += 1
        inner = body[j + 1 : end]
        body = body[:i] + "{\n" + inner + body[t:]
    body = body.replace("Self::checkout_ok(conn,", "Self::checkout_ok(&self.pool,")
    return body


def convert_params_inline(body: str) -> str:
    # multi-line params![ ... ] -> bindings
    while "params![" in body:
        i = body.index("params![")
        j = body.index("]", i)  # naive: first ] - wrong for nested
        # find matching ]
        depth = 0
        end = None
        for k in range(i + len("params!["), len(body)):
            if body[k] == "[":
                depth += 1
            elif body[k] == "]":
                if depth == 0:
                    end = k
                    break
                depth -= 1
        if end is None:
            break
        inner = body[i + len("params![") : end]
        parts = []
        depth = 0
        buf = []
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
        binds = "".join(f".bind({p})" for p in parts)
        body = body[:i] + binds + body[end + 1 :]
    return body


def convert_conn_execute(body: str) -> str:
    body = re.sub(
        r"conn\.execute\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*,",
        r"sqlx::query(\1)",
        body,
    )
    body = convert_params_inline(body)
    body = re.sub(
        r"sqlx::query\((\n?\s*\"(?:[^\"\\]|\\.)*\")\)(\.bind\([^;]+)?\)\?",
        r"sqlx::query(\1)\2.execute(&self.pool).await?",
        body,
    )
    body = re.sub(
        r"sqlx::query\((\n?\s*\"(?:[^\"\\]|\\.)*\")\)(\.bind\([^;]+)?\);",
        r"sqlx::query(\1)\2.execute(&self.pool).await?;",
        body,
    )
    return body


def main():
    raw = SRC.read_text()
    types, body = strip_rusqlite_header(raw)
    body = remove_sqlite_boot(body)
    body = fix_strings(body)
    body = body.replace("rusqlite::Error", "DbError")
    body = unwrap_with_conn(body)
    body = convert_conn_execute(body)

    body = re.sub(r"\n    pub fn ", r"\n    pub async fn ", body)
    body = re.sub(r"\n    fn checkout_ok", r"\n    async fn checkout_ok", body)
    body = re.sub(r"\n    fn insert_checkin_row", r"\n    async fn insert_checkin_row", body)
    body = re.sub(r"\n    fn checkin_replay", r"\n    async fn checkin_replay", body)
    body = re.sub(r"\n    fn latest_device_fixes", r"\n    async fn latest_device_fixes", body)
    body = body.replace("conn: &Connection,", "pool: &PgPool,")

    # bool / register unique
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

    body = body.replace("let tx = conn.unchecked_transaction()?;", "let mut tx = self.pool.begin().await?;")
    # fix append_fixes transaction executes to use &mut *tx - manual

    body = "impl ClubDb {" + body[len("impl ClubDb {") :]

    out = HEADER + types + body
    OUT.write_text(out)
    print("wrote", OUT, "lines", len(out.splitlines()))


if __name__ == "__main__":
    main()