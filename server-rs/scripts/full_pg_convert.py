#!/usr/bin/env python3
"""Convert sqlite ClubDb (db_sqlite_source.rs) + db_access.rs -> async sqlx Postgres."""
from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src/club/db_sqlite_source.rs"
ACCESS = ROOT / "src/club/db_access.rs"
OUT = ROOT / "src/club/db.rs"
OUT_ACCESS = ROOT / "src/club/db_access.rs"

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
            "TRUNCATE TABLE audit_log, access_requests, checkins, device_pair_tokens, flight_devices, fixes, flights, \\
             devices, checkouts, pilot_sessions, admin_map_prefs, display_sessions, \\
             pilots, aircraft, aircraft_types, clubs RESTART IDENTITY CASCADE",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

"""

ACCESS_HEADER = """use crate::club::access_request::{
    normalize_tail, AccessRequestError, AccessRequestView, AdminAccessRequestView,
};
use crate::club::auth::new_id;
use crate::club::db::{now_ms, CheckoutError, ClubDb, DbError};
use serde::Serialize;
use sqlx::{PgPool, Row};
use std::borrow::Cow;

"""


def sql_pg_strings(text: str) -> str:
    def fix(m: re.Match[str]) -> str:
        s = m.group(1)
        if re.search(
            r"\b(SELECT|INSERT|UPDATE|DELETE|FROM|WHERE|VALUES|TRUNCATE|JOIN|ON CONFLICT|ALTER)\b",
            s,
        ):
            s = re.sub(r"\?(\d+)", r"$\1", s)
            s = s.replace("INSERT OR IGNORE INTO fixes", "INSERT INTO fixes")
            s = s.replace("ON CONFLICT(pilot_id)", "ON CONFLICT (pilot_id)")
        return '"' + s + '"'

    return re.sub(r'"((?:[^"\\]|\\.)*)"', fix, text)


def strip_sqlite_plumbing(body: str) -> str:
    body = re.sub(r"const MIGRATION_\d+:.*\n", "", body)
    body = re.sub(
        r"    pub fn open\(data_dir: &Path\)[\s\S]*?    fn bootstrap\(&self\)",
        "    async fn bootstrap(&self)",
        body,
        count=1,
    )
    body = re.sub(
        r"    #\[cfg\(test\)\]\n    pub fn open_in_memory\(\)[\s\S]*?    fn with_conn",
        "    fn _removed_with_conn",
        body,
        count=1,
    )
    body = re.sub(
        r"    fn _removed_with_conn<F, T>\([\s\S]*?\n    \}\n\n",
        "",
        body,
        count=1,
    )
    body = re.sub(
        r"    fn migrate\(&self\)[\s\S]*?\n        \}\)\n    \}\n\n",
        "",
        body,
        count=1,
    )
    return body


def unwrap_with_conn_brace(text: str) -> str:
    needle = "self.with_conn(|conn| {"
    while needle in text:
        i = text.index(needle)
        brace = i + len(needle) - 1
        depth = 0
        end = None
        for j in range(brace, len(text)):
            if text[j] == "{":
                depth += 1
            elif text[j] == "}":
                depth -= 1
                if depth == 0:
                    end = j
                    break
        if end is None:
            break
        tail = end + 1
        if tail < len(text) and text[tail] == ")":
            tail += 1
        inner = text[brace + 1 : end]
        text = text[:i] + "{\n" + inner + text[tail:]
    return text


def unwrap_with_conn_paren(text: str) -> str:
    while True:
        idx = text.find("with_conn(|conn|")
        if idx < 0:
            break
        start = text.rfind("self", 0, idx)
        if start < 0 or text[start:idx].strip() != "self":
            if text[idx - 1] == ".":
                start = idx - 1
            else:
                break
        paren_open = text.index("(", idx)
        depth = 0
        close = None
        for k in range(paren_open, len(text)):
            if text[k] == "(":
                depth += 1
            elif text[k] == ")":
                depth -= 1
                if depth == 0:
                    close = k
                    break
        if close is None:
            break
        inner = text[paren_open + 1 : close].strip()
        if inner.startswith("|conn|"):
            inner = inner[6:].strip()
        if inner.startswith("{") and inner.endswith("}"):
            bdepth = 0
            for j, c in enumerate(inner):
                if c == "{":
                    bdepth += 1
                elif c == "}":
                    bdepth -= 1
                    if bdepth == 0:
                        inner = inner[1:j]
                        break
        rep_start = start if text[start:idx].strip() == "self" else idx
        text = text[:rep_start] + inner + text[close + 1 :]
    return text


def unwrap_with_conn(text: str) -> str:
    text = unwrap_with_conn_brace(text)
    text = unwrap_with_conn_paren(text)
    text = text.replace("Self::checkout_ok(conn,", "Self::checkout_ok(&self.pool,")
    text = text.replace("Self::checkout_ok(pool,", "Self::checkout_ok(&self.pool,")
    return text


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


def convert_executes(text: str) -> str:
    for trail in ("?", "?", ";"):
        pat = re.compile(
            r"conn\.execute\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*,\s*(params!\[[\s\S]*?\])\s*\)\s*"
            + ("?" if trail == "?" else ";" if trail == ";" else r"\?"),
            re.MULTILINE,
        )

        def repl(m: re.Match[str], t=trail) -> str:
            end = "?" if t == "?" else ";"
            ra = ".rows_affected()" if "let updated = sqlx" in m.string[: m.start()] else ""
            if m.string[m.start() - 20 : m.start()].strip().endswith("let updated ="):
                return f"let updated = sqlx::query({m.group(1).strip()}){params_to_bind(m.group(2))}.execute(&self.pool).await?.rows_affected(){end}"
            if m.string[m.start() - 10 : m.start()].strip().endswith("let changed ="):
                return f"let changed = sqlx::query({m.group(1).strip()}){params_to_bind(m.group(2))}.execute(&self.pool).await?.rows_affected(){end}"
            return f"sqlx::query({m.group(1).strip()}){params_to_bind(m.group(2))}.execute(&self.pool).await{end}"

        text = pat.sub(repl, text)
    pat = re.compile(
        r"conn\.execute\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*,\s*\[\]\s*\)\s*\?",
        re.MULTILINE,
    )
    text = pat.sub(
        lambda m: f"sqlx::query({m.group(1).strip()}).execute(&self.pool).await?",
        text,
    )
    return text


def convert_query_row_scalar(text: str) -> str:
    patterns = [
        (
            r"conn\.query_row\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*,\s*(params!\[[\s\S]*?\])\s*,\s*\|r\|\s*r\.get(?:::<_,\s*(\w+)>\()?(\d+)\)?\s*\)\s*\?",
            True,
        ),
        (
            r"conn\.query_row\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*,\s*\[\]\s*,\s*\|r\|\s*r\.get(?:::<_,\s*(\w+)>\()?(\d+)\)?\s*\)\s*\?",
            False,
        ),
    ]
    for pat, has_params in patterns:
        rx = re.compile(pat, re.MULTILINE)

        def repl(m: re.Match[str], hp=has_params) -> str:
            sql = m.group(1).strip()
            if hp:
                binds = params_to_bind(m.group(2))
                ty = m.group(3) or "_"
                idx = m.group(4)
            else:
                binds = ""
                ty = m.group(2) or "i64"
                idx = m.group(3)
            if ty == "_":
                return f"sqlx::query_scalar::<_, i64>({sql}){binds}.fetch_one(&self.pool).await?"
            return f"sqlx::query({sql}){binds}.fetch_one(&self.pool).await?.try_get::<_, {ty}>({idx})?"

        text = rx.sub(repl, text)
    return text


def convert_query_row_optional(text: str) -> str:
    pat = re.compile(
        r"conn\.query_row\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*,\s*(params!\[[\s\S]*?\])\s*,\s*\|r\|\s*(?:r\.)?([\s\S]*?)\s*\)\s*\.optional\(\)",
        re.MULTILINE,
    )

    def repl(m: re.Match[str]) -> str:
        sql = m.group(1).strip()
        binds = params_to_bind(m.group(2))
        body = m.group(3)
        if "r.get::<_, String>(0)" in body and "Ok(r.get" in body:
            return f"sqlx::query_scalar::<_, String>({sql}){binds}.fetch_optional(&self.pool).await"
        if "Ok((r.get" in body or "Ok((r.get" in body:
            gets = re.findall(r"r\.get(?:::<_,\s*(\w+)>\()?(\d+)\)?", body)
            if gets:
                lines = []
                for i, (ty, idx) in enumerate(gets):
                    ty = ty or "_"
                    lines.append(f"let v{i} = row.try_get::<_, {ty}>({idx})?;")
                tup = ", ".join(f"v{i}" for i in range(len(gets)))
                inner = "; ".join(lines) + f"; Ok(({tup}))"
                return (
                    f"{{\n            let row = sqlx::query({sql}){binds}.fetch_optional(&self.pool).await?;\n            "
                    + "row.map(|row| -> Result<_, DbError> { "
                    + inner
                    + " }).transpose()"
                )
        if "PilotRow {" in body:
            return (
                f"sqlx::query_as!(PilotRow, {sql.lstrip(chr(34)).rstrip(chr(34))})"
            )
        return f"sqlx::query({sql}){binds}.fetch_optional(&self.pool).await"

    return pat.sub(repl, text)


def convert_query_row_tuple(text: str) -> str:
    pat = re.compile(
        r"let \(([^)]+)\)(?:: [^=]+)? = conn\.query_row\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*,\s*(params!\[[\s\S]*?\])\s*,\s*\|r\| Ok\(\(([^)]+)\)\)\s*\)\?",
        re.MULTILINE,
    )

    def repl(m: re.Match[str]) -> str:
        names = m.group(1)
        sql = m.group(2).strip()
        binds = params_to_bind(m.group(3))
        inner = m.group(4)
        idxs = re.findall(r"r\.get(?:::<_,\s*(\w+)>\()?(\d+)\)?", inner)
        lines = [f"let {names.split(',')[i].strip()} = row.try_get({idx})?;" for i, (_, idx) in enumerate(idxs)]
        return (
            f"let row = sqlx::query({sql}){binds}.fetch_one(&self.pool).await?;\n        "
            + "\n        ".join(lines)
        )

    return pat.sub(repl, text)


def convert_prepare_query_map(text: str) -> str:
    pat = re.compile(
        r"let mut stmt = conn\.prepare\(\s*(\n?\s*\"(?:[^\"\\]|\\.)*\")\s*\)\?;\s*"
        r"let rows = stmt\.query_map\(\s*(params!\[[\s\S]*?\]|\[\])\s*,\s*\|r\|\s*([\s\S]*?)\)\?;\s*"
        r"((?:let mut out = Vec::new\(\);\s*)?"
        r"for row in rows \{\s*([\s\S]*?)\s*\}\s*)"
        r"(Ok\([\s\S]*?\))",
        re.MULTILINE,
    )

    struct_map = {
        "LiveFix {": "t: row.try_get(0)?, lat: row.try_get(1)?, lon: row.try_get(2)?, alt: row.try_get(3)?, alt_ref: row.try_get(4).ok(), speed: row.try_get(5)?, course: row.try_get(6)?, accuracy: row.try_get(7)?",
        "DeviceInfo {": "id: row.try_get(0)?, label: row.try_get(1)?, created_at: row.try_get(2)?",
        "TypeRow {": "id: row.try_get(0)?, code: row.try_get(1)?",
        "AircraftRow {": "id: row.try_get(0)?, tail: row.try_get(1)?, type_code: row.try_get(2)?, active: row.try_get(3)?",
        "FlightSummary {": "id: row.try_get(0)?, tail: row.try_get(1)?, type_code: row.try_get(2)?, started_at: row.try_get(3)?, ended_at: row.try_get(4)?, visibility: row.try_get(5)?",
        "CheckoutRow {": "id: row.try_get(0)?, pilot_name: row.try_get(1)?, type_code: row.try_get(2)?, tail: row.try_get(3)?, expires_at: row.try_get(4)?",
        "DisplaySessionRow {": "id: row.try_get(0)?, label: row.try_get(1)?, created_at: row.try_get(2)?, revoked: row.try_get::<_, Option<i64>>(3)?.is_some()",
        "PilotRow {": "id: row.try_get(0)?, club_id: row.try_get(1)?, name: row.try_get(2)?, email: row.try_get(3)?, license_no: row.try_get(4)?, status: row.try_get(5)?, role: row.try_get(6)?",
        "TailInfo {": "id: row.try_get(0)?, tail: row.try_get(1)?, type_code: row.try_get(2)?",
        "DeviceFix {": "device_id: row.try_get(0)?, tail: row.try_get(1)?, flight_id: row.try_get(2)?, pilot_name: row.try_get(3)?, show_pilot_name_public: row.try_get(4)?, visibility: row.try_get(5)?, t_ms: row.try_get(6)?, lat: row.try_get(7)?, lon: row.try_get(8)?, alt: row.try_get(9)?, speed: row.try_get(10)?, course: row.try_get(11)?, accuracy: row.try_get(12)?",
    }

    def repl(m: re.Match[str]) -> str:
        sql = m.group(1).strip()
        params = m.group(2).strip()
        loop_body = m.group(4)
        tail = m.group(5)
        binds = params_to_bind(params) if params.startswith("params!") else ""
        for key, fields in struct_map.items():
            if key in loop_body:
                stype = key.split()[0]
                return (
                    f"let rows = sqlx::query({sql}){binds}.fetch_all(&self.pool).await?;\n"
                    f"            let mut out = Vec::new();\n"
                    f"            for row in rows {{\n"
                    f"                out.push({stype} {{ {fields} }});\n"
                    f"            }}\n"
                    f"            {tail}"
                )
        if "Self::checkout_ok" in loop_body:
            return (
                f"let rows = sqlx::query({sql}){binds}.fetch_all(&self.pool).await?;\n"
                f"            let mut out = Vec::new();\n"
                f"            for row in rows {{\n"
                f"                let (id, tail, type_code) = (row.try_get::<_, String>(0)?, row.try_get::<_, String>(1)?, row.try_get::<_, String>(2)?);\n"
                f"                if Self::checkout_ok(&self.pool, pilot_id, &id, now).await? {{\n"
                f"                    out.push(TailInfo {{ id, tail, type_code }});\n"
                f"                }}\n"
                f"            }}\n"
                f"            Ok(out)"
            )
        return f"/*PREPARE*/{m.group(0)[:80]}"

    return pat.sub(repl, text)


def patch_common(body: str) -> str:
    body = body.replace("rusqlite::Error", "DbError")
    body = body.replace("RegisterError::Db(#[allow(dead_code)] rusqlite::Error)", "RegisterError::Db(#[allow(dead_code)] sqlx::Error)")
    body = body.replace("RegisterError::Db(#[allow(dead_code)] DbError)", "RegisterError::Db(#[allow(dead_code)] sqlx::Error)")
    body = body.replace("conn: &Connection,", "pool: &PgPool,")
    body = body.replace("conn.query_row", "CONN_QR")
    body = body.replace("conn.execute", "CONN_EX")
    body = body.replace("conn.prepare", "CONN_PR")
    body = body.replace("CONN_QR", "conn.query_row")
    body = body.replace("CONN_EX", "conn.execute")
    body = body.replace("CONN_PR", "conn.prepare")
    body = body.replace("i64::from(show_pilot_name_public)", "show_pilot_name_public")
    body = body.replace("i64::from(flag)", "flag")
    body = body.replace("i64::from(active)", "active")
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
    body = body.replace("tx.commit()?;", "tx.commit().await?;")
    body = body.replace(".execute(&self.pool).await?.rows_affected()", ".execute(&mut *tx).await?.rows_affected()")
    # revert double tx execute only inside begin blocks - heuristic: only when tx exists in method
    return body


def patch_awaits(body: str) -> str:
    calls = [
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
        "self.append_fixes(",
        "self.mint_checkin_qr_token(",
        "self.default_club_id(",
        "self.pilot_from_session(",
        "self.pilot_has_open_checkin(",
        "Self::checkout_ok(",
        "self.record_audit(",
        "self.approve_access_request(",
        "self.count_pending_access_requests(",
        "self.list_pending_pilots(",
    ]
    for c in calls:
        body = re.sub(rf"({re.escape(c)}[^;\n]+)(\);)", r"\1.await\2", body)
        body = re.sub(rf"({re.escape(c)}[^?\n]+)(\)\?)", r"\1.await\2", body)
    body = body.replace(".await.await", ".await")
    return body


def patch_append_fixes(body: str) -> str:
    body = body.replace(
        '"INSERT INTO fixes (flight_id, device_id, t_ms, lat, lon, alt, speed, course, accuracy)',
        '"INSERT INTO fixes (flight_id, device_id, t_ms, received_at, lat, lon, alt, alt_ref, speed, course, accuracy)',
    )
    body = re.sub(
        r'params!\[\s*flight_id,\s*device_id,\s*fix\.t,',
        "params![flight_id, device_id, fix.t, now_ms(),",
        body,
    )
    body = body.replace(
        "VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        "VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) ON CONFLICT (flight_id, device_id, t_ms) DO NOTHING",
    )
    body = body.replace(
        "fix.accuracy\n                    ],",
        "fix.alt_ref.as_deref(), fix.speed, fix.course, fix.accuracy\n                    ],",
    )
    return body


def transform_impl(impl_body: str, is_access: bool = False) -> str:
    impl_body = strip_sqlite_plumbing(impl_body) if not is_access else impl_body
    impl_body = sql_pg_strings(impl_body)
    impl_body = unwrap_with_conn(impl_body)
    impl_body = convert_executes(impl_body)
    impl_body = convert_query_row_scalar(impl_body)
    impl_body = convert_query_row_tuple(impl_body)
    impl_body = convert_query_row_optional(impl_body)
    impl_body = convert_prepare_query_map(impl_body)
    impl_body = patch_common(impl_body)
    impl_body = patch_append_fixes(impl_body)
    impl_body = re.sub(r"\n    pub fn ", r"\n    pub async fn ", impl_body)
    for fn in ("bootstrap", "checkout_ok", "insert_checkin_row", "checkin_replay", "latest_device_fixes"):
        impl_body = re.sub(rf"\n    fn {fn}\(", rf"\n    async fn {fn}(", impl_body)
    if is_access:
        impl_body = impl_body.replace("rusqlite::Row<'_>", "sqlx::postgres::PgRow")
        impl_body = impl_body.replace("rusqlite::Result", "Result<(), DbError>")
        impl_body = impl_body.replace(
            "fn map_access_row(\n        r: &sqlx::postgres::PgRow,\n        include_pilot: bool,\n    ) -> Result<(), DbError><AdminAccessRequestView>",
            "fn map_access_row(\n        r: &sqlx::postgres::PgRow,\n        include_pilot: bool,\n    ) -> Result<AdminAccessRequestView, DbError>",
        )
        impl_body = impl_body.replace("r.get::<_", "r.try_get::<_")
    impl_body = patch_awaits(impl_body)
    return impl_body


def convert_tests(text: str) -> str:
    text = text.replace("ClubDb::open_in_memory().unwrap()", "ClubDb::open_test().await.unwrap()")
    text = re.sub(r"#\[test\]\n    fn ", "#[tokio::test]\n    async fn ", text)
    text = unwrap_with_conn_brace(text)
    text = unwrap_with_conn_paren(text)
    text = text.replace("db.with_conn(|conn| {", "{")
    text = re.sub(
        r'conn\.execute\(\s*"([^"]+)"\s*,\s*params!\[([^\]]+)\]\s*\)',
        lambda m: f'sqlx::query("{sql_pg_strings(chr(34)+m.group(1)+chr(34)).strip(chr(34))}").bind({m.group(2)}).execute(&db.pool).await',
        text,
    )
    text = re.sub(
        r'conn\.query_row\(\s*"([^"]+)"\s*,\s*params!\[([^\]]+)\]\s*,\s*\|r\|\s*Ok\(\(r\.get\(0\)\?, r\.get\(1\)\?\)\)\s*\)',
        lambda m: (
            f'{{ let row = sqlx::query("{re.sub(r"\\?(\\d+)", r"$\\1", m.group(1))}").bind({m.group(2)}).fetch_one(&db.pool).await.unwrap(); (row.try_get(0).unwrap(), row.try_get(1).unwrap()) }}'
        ),
        text,
    )
    text = re.sub(
        r'conn\.query_row\(\s*"([^"]+)"\s*,\s*params!\[([^\]]+)\]\s*,\s*\|r\|\s*r\.get\(0\)\s*\)',
        lambda m: f'sqlx::query_scalar::<_, i64>("{re.sub(r"\\?(\\d+)", r"$\\1", m.group(1))}").bind({m.group(2)}).fetch_one(&db.pool).await.unwrap()',
        text,
    )
    text = patch_awaits(text)
    return text


def main() -> None:
    raw = SRC.read_text()
    ts = raw.index("pub struct PilotRow")
    impl_i = raw.index("impl ClubDb {")
    types = raw[ts:impl_i].replace("rusqlite::Error", "DbError")
    impl_rest = raw[impl_i:]
    tests_split = impl_rest.find("\npub fn now_ms")
    impl_only = impl_rest[:tests_split] if tests_split > 0 else impl_rest
    tail = impl_rest[tests_split:] if tests_split > 0 else ""
    impl_only = transform_impl(impl_only)
    impl_only = impl_only.replace("impl ClubDb {", "impl ClubDb {" + CONNECT, 1)
    tail = convert_tests(tail.replace("rusqlite::Error", "sqlx::Error"))

    out = HEADER + types + impl_only + tail
    if "conn." in out or "with_conn" in out or "params!" in out:
        print(
            "WARN leftovers:",
            "conn", out.count("conn."),
            "with_conn", out.count("with_conn"),
            "params", out.count("params!"),
        )
    OUT.write_text(out)

    access_raw = ACCESS.read_text()
    access_impls = []
    for m in re.finditer(r"impl ClubDb \{([\s\S]*?)\n\}", access_raw):
        access_impls.append(transform_impl("impl ClubDb {" + m.group(1) + "\n}", is_access=True))
    access_out = ACCESS_HEADER
    for block in access_impls:
        access_out += block + "\n"
    # keep AdminPendingCounts struct block
    if "pub struct AdminPendingCounts" in access_raw:
        access_out += "\n" + access_raw[access_raw.index("pub struct AdminPendingCounts") :]
    OUT_ACCESS.write_text(access_out)
    print("wrote", OUT, "and", OUT_ACCESS)


if __name__ == "__main__":
    main()