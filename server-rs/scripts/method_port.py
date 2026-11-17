#!/usr/bin/env python3
"""Per-method sqlite -> sqlx port (conservative)."""
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

pub const MAX_DEVICES_PER_PILOT: usize = 10;
pub const MIN_INSTALL_ID_LEN: usize = 8;
pub const MAX_INSTALL_ID_LEN: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceRegisterError {
    InvalidInstallId,
    CapExceeded,
    Db,
}

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


def pg_sql(s: str) -> str:
    return re.sub(r"\?(\d+)", r"$\1", s)


def find_matching(text: str, start: int, open_c: str, close_c: str) -> int:
    depth = 0
    for i in range(start, len(text)):
        if text[i] == open_c:
            depth += 1
        elif text[i] == close_c:
            depth -= 1
            if depth == 0:
                return i
    raise ValueError("unbalanced")


def split_params_list(inner: str) -> list[str]:
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


def unwrap_with_conn(body: str) -> str:
    needle = "self.with_conn(|conn| {"
    while needle in body:
        i = body.index(needle)
        brace = i + len(needle) - 1
        end = find_matching(body, brace, "{", "}")
        tail = end + 1
        if tail < len(body) and body[tail] == ")":
            tail += 1
        inner = body[brace + 1 : end]
        body = body[:i] + inner + body[tail:]
    while "self.with_conn(|conn|" in body:
        i = body.index("self.with_conn(|conn|")
        paren = body.index("(", i)
        close = find_matching(body, paren, "(", ")")
        inner = body[paren + 1 : close].strip()
        if inner.startswith("|conn|"):
            inner = inner[6:].strip()
        if inner.startswith("{") and inner.endswith("}"):
            b = find_matching(inner, 0, "{", "}")
            inner = inner[1:b]
        body = body[:i] + inner + body[close + 1 :]
    body = re.sub(r"\bself\.\s*\n", "", body)
    body = re.sub(r"match\s+self\.\s*\n", "match ", body)
    body = body.replace("Self::checkout_ok(conn,", "Self::checkout_ok(&self.pool,")
    return body


def replace_params(sql: str, bind_expr: str) -> str:
    binds = "".join(f".bind({p})" for p in split_params_list(bind_expr))
    return f'sqlx::query("{pg_sql(sql)}"){binds}'


def convert_executes(body: str) -> str:
    out = []
    i = 0
    while i < len(body):
        j = body.find("conn.execute(", i)
        if j < 0:
            out.append(body[i:])
            break
        out.append(body[i:j])
        paren = j + len("conn.execute(") - 1
        close = find_matching(body, paren, "(", ")")
        chunk = body[paren + 1 : close]
        # parse sql and params
        q = chunk.find('"')
        if q < 0:
            out.append(body[j : close + 1])
            i = close + 1
            continue
        qend = q + 1
        while qend < len(chunk):
            if chunk[qend] == '"' and chunk[qend - 1] != "\\":
                break
            qend += 1
        sql = chunk[q + 1 : qend]
        rest = chunk[qend + 1 :].strip()
        trailing = ""
        if rest.startswith(","):
            rest = rest[1:].strip()
            if rest.startswith("params!["):
                pb = find_matching(rest, rest.index("["), "[", "]")
                bind = rest[rest.index("[") + 1 : pb]
                converted = replace_params(sql, bind) + ".execute(&self.pool).await"
                rest_after = rest[pb + 1 :].strip()
                if rest_after.startswith(")?"):
                    converted += "?"
                    rest_after = rest_after[2:].strip()
                elif rest_after.startswith(");"):
                    converted += ";"
                    rest_after = rest_after[2:].strip()
                elif rest_after.startswith(")"):
                    converted += ""
                    rest_after = rest_after[1:].lstrip(",").strip()
                converted = converted.replace(".await,", ".await")
                out.append(converted)
                if rest_after.startswith("?"):
                    out.append("?")
                    rest_after = rest_after[1:]
                trailing = rest_after
            elif rest.startswith("[]"):
                converted = f'sqlx::query("{pg_sql(sql)}").execute(&self.pool).await'
                rest_after = rest[2:].strip()
                if rest_after.startswith(")?"):
                    converted += "?"
                out.append(converted)
                trailing = rest_after.lstrip(")?").strip()
            else:
                out.append(body[j : close + 1])
        else:
            out.append(body[j : close + 1])
        i = close + 1 + len(trailing)
        if trailing:
            out.append(trailing)
    return "".join(out)


def convert_query_row_scalar(body: str) -> str:
    """conn.query_row("...", params![..], |r| r.get(0))"""
    pat = re.compile(
        r'conn\.query_row\(\s*"([^"]+)"\s*,\s*params!\[([^\]]*)\]\s*,\s*\|r\|\s*r\.get(?:::<_,\s*(\w+)>\()?(\d+)\)?\s*\)\??',
    )
    body = pat.sub(
        lambda m: (
            f'sqlx::query_scalar::<_, {m.group(3) or "i64"}>("{pg_sql(m.group(1))}")'
            + "".join(f".bind({p})" for p in split_params_list(m.group(2)))
            + ".fetch_one(&self.pool).await?"
        ),
        body,
    )
    pat2 = re.compile(
        r'conn\.query_row\(\s*"([^"]+)"\s*,\s*\[\]\s*,\s*\|r\|\s*r\.get(?:::<_,\s*(\w+)>\()?(\d+)\)?\s*\)\??',
    )
    body = pat2.sub(
        lambda m: f'sqlx::query_scalar::<_, {m.group(2) or "i64"}>("{pg_sql(m.group(1))}").fetch_one(&self.pool).await?',
        body,
    )
    return body


def port_body(body: str) -> str:
    import importlib.util
    spec = importlib.util.spec_from_file_location("fpc", ROOT / "scripts/full_pg_convert.py")
    fpc = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fpc)
    body = body.replace("rusqlite::Error", "DbError")
    for m in re.finditer(r'"([^"]*(?:SELECT|INSERT|UPDATE|DELETE|FROM)[^"]*)"', body):
        old = m.group(0)
        body = body.replace(old, f'"{pg_sql(m.group(1))}"', 1)
    body = unwrap_with_conn(body)
    body = convert_executes(body)
    body = convert_query_row_scalar(body)
    body = body.replace("i64::from(show_pilot_name_public)", "show_pilot_name_public")
    body = body.replace("(type_id, active): (String, i64)", "(type_id, active): (String, bool)")
    body = body.replace("if active == 0", "if !active")
    body = body.replace("conn: &Connection,", "pool: &PgPool,")
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
    body = body.replace("let tx = conn.unchecked_transaction()?;", "let mut tx = self.pool.begin().await?;")
    body = body.replace("tx.commit()?;", "tx.commit().await?;")
    body = body.replace(".execute(&self.pool).await,;", ".execute(&self.pool).await?;")
    body = body.replace(".execute(&self.pool).await,", ".execute(&self.pool).await?")
    body = fpc.convert_query_row_optional(body)
    body = fpc.convert_query_row_tuple(body)
    body = fpc.convert_prepare_query_map(body)
    body = body.replace("INSERT OR IGNORE INTO fixes", "INSERT INTO fixes")
    body = body.replace(
        '"INSERT INTO fixes (flight_id, device_id, t_ms, lat, lon, alt, speed, course, accuracy)',
        '"INSERT INTO fixes (flight_id, device_id, t_ms, received_at, lat, lon, alt, alt_ref, speed, course, accuracy)',
    )
    return body


def extract_fns(impl_text: str) -> list[str]:
    fns = []
    i = 0
    while True:
        m = re.search(r"\n    ((?:pub )?(?:async )?fn \w+)", impl_text[i:])
        if not m:
            break
        start = i + m.start()
        brace = impl_text.find("{", start)
        end = find_matching(impl_text, brace, "{", "}")
        fns.append(impl_text[start : end + 1])
        i = end + 1
    return fns


def main() -> None:
    raw = SRC.read_text()
    acc = ROOT / "src/club/db_access.rs"
    if acc.exists() and "rusqlite" in acc.read_text():
        acc_text = acc.read_text()
        for m in re.finditer(r"impl ClubDb \{([\s\S]*?)\n\}", acc_text):
            raw = raw.replace(
                "\n}\n\npub fn now_ms",
                "\n" + m.group(1) + "\n}\n\npub fn now_ms",
                1,
            )
    ts = raw.index("pub struct PilotRow")
    impl_i = raw.index("impl ClubDb {")
    types = raw[ts:impl_i].replace("rusqlite::Error", "DbError")
    impl_text = raw[impl_i + len("impl ClubDb {") :]
    tests_i = impl_text.find("\n}\n\npub fn now_ms")
    if tests_i < 0:
        tests_i = impl_text.rfind("\n}\n")
    core = impl_text[:tests_i]
    tail = impl_text[tests_i + 3 :] if tests_i > 0 else ""

    skip = {"open", "open_in_memory", "with_conn", "migrate"}
    ported = []
    manual = []
    for fn_block in extract_fns(core):
        name_m = re.search(r"fn (\w+)", fn_block)
        name = name_m.group(1) if name_m else "?"
        if name in skip:
            continue
        out = port_body(fn_block)
        if "conn." in out or "params!" in out or "with_conn" in out:
            manual.append(name)
        out = re.sub(r"\n    pub fn ", "\n    pub async fn ", out, count=1)
        out = re.sub(r"\n    fn ", "\n    async fn ", out, count=1)
        ported.append(out)

    header = HEADER.replace("    pool: PgPool,", "    pub(crate) pool: PgPool,")
    impl = "impl ClubDb {\n" + CONNECT + "\n".join(ported) + "\n}\n"
    impl = impl.replace(
        "impl ClubDb {",
        'impl ClubDb {\n    pub(crate) fn data_dir(&self) -> &std::path::Path {\n        static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();\n        DIR.get_or_init(|| std::env::var("FLIGHT_DATA").map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::PathBuf::from("data")))\n    }\n',
        1,
    )
    tail = tail.replace("ClubDb::open_in_memory().unwrap()", "ClubDb::open_test().await.unwrap()")
    tail = re.sub(r"#\[test\]\n    fn ", "#[tokio::test]\n    async fn ", tail)
    tail = tail.replace("rusqlite::Error", "sqlx::Error")
    tail = port_body(tail)

    out = header + types + impl + tail
    print("manual methods:", ", ".join(manual[:40]), f"({len(manual)} total)")
    OUT.write_text(out)
    print("wrote", OUT)


if __name__ == "__main__":
    main()