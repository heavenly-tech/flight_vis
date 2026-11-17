#!/usr/bin/env python3
"""Build async sqlx db.rs from db_sqlite_reference.rs."""
from __future__ import annotations

import re
from pathlib import Path

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


def extract_methods(impl_text: str) -> list[tuple[str, str, str]]:
    """Return (full_header, name, body) for each fn in impl."""
    pattern = re.compile(r"\n    ((?:pub )?fn \w+[^{]*\{)", re.MULTILINE)
    matches = list(pattern.finditer(impl_text))
    out = []
    for i, m in enumerate(matches):
        start = m.start()
        header = m.group(1)
        name = re.search(r"fn (\w+)", header).group(1)
        if name in ("open", "open_in_memory", "with_conn", "migrate"):
            continue
        brace = m.end() - 1
        depth = 0
        end = None
        for j in range(brace, len(impl_text)):
            if impl_text[j] == "{":
                depth += 1
            elif impl_text[j] == "}":
                depth -= 1
                if depth == 0:
                    end = j + 1
                    break
        if end is None:
            continue
        body = impl_text[m.start() : end]
        out.append((body, name, body))
    return out


def pg_sql(text: str) -> str:
    text = re.sub(r"\?(\d+)", r"$\1", text)
    text = text.replace("INSERT OR IGNORE INTO fixes", "INSERT INTO fixes")
    text = text.replace("ON CONFLICT(pilot_id)", "ON CONFLICT (pilot_id)")
    return text


def fix_sql_literals(text: str) -> str:
    def repl(m: re.Match[str]) -> str:
        s = m.group(1)
        if re.search(r"\b(SELECT|INSERT|UPDATE|DELETE|FROM|WHERE|VALUES|TRUNCATE)\b", s):
            return '"' + pg_sql(s) + '"'
        return m.group(0)

    return re.sub(r'"((?:[^"\\]|\\.)*)"', repl, text)


def split_params(inner: str) -> list[str]:
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
    return parts


def params_to_bind(params: str) -> str:
    params = params.strip()
    if not params.startswith("params!["):
        return ""
    return "".join(f".bind({p})" for p in split_params(params[len("params![") : -1]))


def unwrap_with_conn(text: str) -> str:
    while "self.with_conn(|conn| {" in text:
        i = text.index("self.with_conn(|conn| {")
        j = i + len("self.with_conn(|conn| {") - 1
        depth = 0
        end = None
        for k in range(j, len(text)):
            if text[k] == "{":
                depth += 1
            elif text[k] == "}":
                depth -= 1
                if depth == 0:
                    end = k
                    break
        tail = end + 1
        if tail < len(text) and text[tail] == ")":
            tail += 1
        # handle .map_err after with_conn
        inner = text[j + 1 : end]
        text = text[:i] + "{\n" + inner + text[tail:]
    while ".with_conn(|conn|" in text:
        i = text.index(".with_conn(|conn|")
        paren = text.index("(", i)
        depth = 0
        end_paren = None
        for k in range(paren, len(text)):
            if text[k] == "(":
                depth += 1
            elif text[k] == ")":
                depth -= 1
                if depth == 0:
                    end_paren = k
                    break
        if end_paren is None:
            break
        inner = text[paren + 1 : end_paren].strip()
        if inner.startswith("|conn|"):
            inner = inner[len("|conn|") :].strip()
        if inner.startswith("{"):
            inner = inner[1:-1] if inner.endswith("}") else inner[1:]
        text = text[:i] + inner + text[end_paren + 1 :]
    text = text.replace("Self::checkout_ok(conn,", "Self::checkout_ok(&self.pool,")
    text = text.replace("db.with_conn(|conn| {", "{\n            let pool = &db.pool;\n            ")
    return text


def convert_executes(text: str) -> str:
    text = re.sub(
        r"conn\.execute\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*,\s*(params!\[[\s\S]*?\])\s*\)\?",
        lambda m: f"sqlx::query({m.group(1).strip()}){params_to_bind(m.group(2))}.execute(&self.pool).await?",
        text,
    )
    text = re.sub(
        r"conn\.execute\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*,\s*(params!\[[\s\S]*?\])\s*\)\?;",
        lambda m: f"sqlx::query({m.group(1).strip()}){params_to_bind(m.group(2))}.execute(&self.pool).await?;",
        text,
    )
    text = re.sub(
        r"let updated = conn\.execute\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*,\s*(params!\[[\s\S]*?\])\s*\)\?",
        lambda m: f"let updated = sqlx::query({m.group(1).strip()}){params_to_bind(m.group(2))}.execute(&self.pool).await?.rows_affected()",
        text,
    )
    return text


def convert_scalar_rows(text: str) -> str:
    text = re.sub(
        r"conn\.query_row\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*,\s*\[\]\s*,\s*\|r\|\s*r\.get(?:::<_,\s*(\w+)>\()?(\d+)\)?\s*\)\?",
        lambda m: f"sqlx::query_scalar({m.group(1).strip()}).fetch_one(&self.pool).await?",
        text,
    )
    text = re.sub(
        r"conn\.query_row\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*,\s*(params!\[[\s\S]*?\])\s*,\s*\|r\|\s*r\.get(?:::<_,\s*(\w+)>\()?(\d+)\)?\s*\)\?",
        lambda m: f"sqlx::query_scalar({m.group(1).strip()}){params_to_bind(m.group(2))}.fetch_one(&self.pool).await?",
        text,
    )
    text = re.sub(
        r"conn\.query_row\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*,\s*(params!\[[\s\S]*?\])\s*,\s*\|r\|\s*r\.get\(0\)\s*\)\s*\.optional\(\)",
        lambda m: f"sqlx::query_scalar({m.group(1).strip()}){params_to_bind(m.group(2))}.fetch_optional(&self.pool).await?",
        text,
    )
    return text


def convert_match_execute(text: str) -> str:
    """Fix register-style match { conn.execute ... Ok(id) }."""
    text = re.sub(
        r"match \{\s*\n\s*sqlx::query\((\"[\s\S]*?\")\)\s*\.bind\([\s\S]*?\),\s*\n\s*\)\?;\s*\n\s*Ok\(id\)\s*\n\s*\{",
        lambda m: f"match sqlx::query({m.group(1)})",
        text,
    )
    return text


def transform_method(body: str) -> str:
    body = body.replace("rusqlite::Error", "DbError")
    body = fix_sql_literals(body)
    body = unwrap_with_conn(body)
    body = convert_executes(body)
    body = convert_scalar_rows(body)
    body = body.replace("pub fn ", "pub async fn ")
    body = body.replace("fn bootstrap(", "async fn bootstrap(")
    body = body.replace("fn checkout_ok(", "async fn checkout_ok(")
    body = body.replace("fn insert_checkin_row(", "async fn insert_checkin_row(")
    body = body.replace("fn checkin_replay(", "async fn checkin_replay(")
    body = body.replace("fn latest_device_fixes(", "async fn latest_device_fixes(")
    body = body.replace("conn: &Connection,", "pool: &PgPool,")
    body = body.replace("i64::from(show_pilot_name_public)", "show_pilot_name_public")
    body = body.replace("i64::from(flag)", "flag")
    body = body.replace("i64::from(active)", "active")
    body = body.replace("r.get::<_, i64>(8)? != 0", "row.try_get::<_, bool>(8)?")
    body = body.replace("r.get::<_, i64>(6)? != 0", "row.try_get::<_, bool>(6)?")
    body = body.replace("r.get::<_, i64>(3)? != 0", "row.try_get::<_, bool>(3)?")
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
    body = body.replace("let tx = conn.unchecked_transaction()?;", "let mut tx = self.pool.begin().await?;")
    body = body.replace(
        """if let DbError::SqliteFailure(err, _) = &e {
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
    return body


def patch_viewer_sessions(text: str) -> str:
    text = text.replace(
        "let Some(pilot) = self.pilot_from_session(session_id, now) else",
        "let Some(pilot) = self.pilot_from_session(session_id, now).await else",
    )
    return text


def patch_internal_awaits(text: str) -> str:
    """Add .await on common internal calls (heuristic)."""
    for m in [
        "self.get_flight(",
        "self.bootstrap(",
        "self.club_checkin_settings(",
        "self.ensure_checkin_secret(",
        "self.pilot_club_id(",
        "self.consume_checkin(",
        "self.device_pilot_id(",
        "self.checkin_replay(",
        "self.insert_checkin_row(",
        "self.latest_device_fixes(",
        "self.admin_map_tails(",
        "self.club_map_for_viewer(",
        "self.upsert_type(",
        "self.upsert_aircraft(",
        "self.add_checkout(",
        "self.create_device(",
        "self.issue_open_checkin(",
        "self.start_flight(",
        "self.append_fix(",
        "self.mint_checkin_qr_token(",
        "self.default_club_id(",
    ]:
        text = re.sub(rf"({re.escape(m)}[^;\n]+)(\);)", r"\1.await\2", text)
        text = re.sub(rf"({re.escape(m)}[^?\n]+)(\)\?)", r"\1.await\2", text)
    # fix double await
    text = text.replace(".await.await", ".await")
    return text


def main() -> None:
    raw = SRC.read_text()
    ts = raw.index("pub struct PilotRow")
    impl_i = raw.index("impl ClubDb {")
    types = raw[ts:impl_i].replace("rusqlite::Error", "DbError")
    impl_text = raw[impl_i + len("impl ClubDb {") :]
    methods = extract_methods(impl_text)

    bodies = []
    skip = {"open", "open_in_memory", "with_conn", "migrate"}
    for body, name, _ in methods:
        if name in skip:
            continue
        bodies.append(transform_method(body))

    impl = "impl ClubDb {\n" + CONNECT + "\n".join(bodies) + "\n}\n"
    rest = raw[raw.index("pub fn now_ms") :]
    rest = rest.replace("rusqlite::Error", "sqlx::Error")
    rest = rest.replace("ClubDb::open_in_memory().unwrap()", "ClubDb::open_test().await.unwrap()")
    rest = re.sub(r"#\[test\]\n    fn ", "#[tokio::test]\n    async fn ", rest)
    # test with_conn -> sqlx
    rest = rest.replace("db.with_conn(|conn| {", "{\n            ")
    rest = re.sub(
        r'conn\.execute\(\s*"([^"]+)"\s*,\s*params!\[([^\]]+)\]\s*\)',
        lambda m: f'sqlx::query("{pg_sql(m.group(1))}").bind({m.group(2)}).execute(&db.pool).await',
        rest,
    )
    rest = re.sub(
        r'conn\.query_row\(\s*"([^"]+)"\s*,\s*params!\[([^\]]+)\]\s*,',
        lambda m: f'sqlx::query_scalar("{pg_sql(m.group(1))}").bind({m.group(2)}).fetch_one(&db.pool).await',
        rest,
    )

    out = HEADER + types + impl + rest
    out = patch_viewer_sessions(out)
    out = patch_internal_awaits(out)
    OUT.write_text(out)
    if "conn." in out or "with_conn" in out or "params!" in out:
        print("WARN leftovers:", "conn", out.count("conn."), "with_conn", out.count("with_conn"), "params", out.count("params!"))
    print("wrote", OUT)


if __name__ == "__main__":
    main()