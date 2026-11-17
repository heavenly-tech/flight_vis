#!/usr/bin/env python3
"""Replace known-good sqlx method bodies in db.rs."""
from __future__ import annotations

import importlib.util
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DB = ROOT / "src/club/db.rs"
SNIP = ROOT / "scripts/snippets"

spec = importlib.util.spec_from_file_location("mp", ROOT / "scripts/method_port.py")
mp = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mp)


def replace_in_db(db: str, name: str, new_body: str) -> str:
    for pat in (
        rf"\n    pub async fn {name}\(",
        rf"\n    async fn {name}\(",
        rf"\n    pub fn {name}\(",
        rf"\n    pub\(crate\) async fn {name}\(",
        rf"\n    pub\(crate\) fn {name}\(",
    ):
        m = re.search(pat, db)
        if m:
            start = m.start() + 1
            brace = db.find("{", start)
            end = mp.find_matching(db, brace, "{", "}")
            return db[:start] + new_body.strip() + db[end + 1 :]
    raise KeyError(name)


def replace_from_bootstrap_through_pilot(db: str, snippet: str) -> str:
    start = db.index("    async fn bootstrap(&self)")
    m = re.search(r"\n    pub async fn (?:approve_pilot|start_flight|me_tails|register)\(", db[start:])
    if not m:
        m = re.search(r"\n    pub async fn register\(", db[start:])
    if not m:
        raise ValueError("register not found after bootstrap")
    # find pilot_from_session end
    m2 = re.search(r"\n    pub async fn pilot_from_session\(", db[start:])
    if not m2:
        raise ValueError("pilot_from_session not found")
    brace = db.find("{", start + m2.start())
    end = mp.find_matching(db, brace, "{", "}")
    # keep from next method after pilot_from_session
    tail_start = end + 1
    while tail_start < len(db) and db[tail_start] in " \n":
        tail_start += 1
    return db[:start] + snippet.strip() + "\n\n" + db[tail_start:]


def main() -> None:
    db = DB.read_text()
    auth = (ROOT / "scripts/auth_core.rs.snippet").read_text()
    db = replace_from_bootstrap_through_pilot(db, auth)

    for name, fname in [
        ("approve_pilot", "flight_core.rs"),
        ("checkout_ok", "checkout_ok.rs"),
        ("me_tails", "flight_core.rs"),
        ("start_flight_atomic", "start_flight_atomic.rs"),
        ("start_flight", "patch_through_append.rs"),
        ("patch_flight", "patch_through_append.rs"),
        ("end_flight", "patch_through_append.rs"),
        ("device_pilot_id", "patch_through_append.rs"),
        ("resolve_device_token", "patch_through_append.rs"),
        ("open_flight_for_device", "patch_through_append.rs"),
        ("append_fix", "patch_through_append.rs"),
        ("append_fixes", "patch_through_append.rs"),
    ]:
        snip = (SNIP / fname).read_text()
        # extract single method from multi-method snippet files
        for pat in (rf"    pub async fn {name}\(", rf"    pub\(crate\) async fn {name}\("):
            m = re.search(pat, snip)
            if m:
                brace = snip.find("{", m.start())
                end = mp.find_matching(snip, brace, "{", "}")
                body = snip[m.start() : end + 1]
                try:
                    db = replace_in_db(db, name, body)
                except KeyError:
                    if name == "checkout_ok":
                        ins = db.index("    pub async fn me_tails(")
                        db = db[:ins] + body + "\n\n" + db[ins:]
                    elif name == "start_flight_atomic":
                        ins = db.index("    pub async fn start_flight(")
                        db = db[:ins] + body + "\n\n" + db[ins:]
                    else:
                        raise
                print("injected", name)
                break
        else:
            if name == "checkout_ok":
                m = re.search(r"    pub\(crate\) async fn checkout_ok\(", snip)
                if not m:
                    m = re.search(
                        r"    pub\(crate\) async fn checkout_ok\(",
                        (ROOT / "scripts/snippets/checkout_ok.rs").read_text(),
                    )
                    snip = (ROOT / "scripts/snippets/checkout_ok.rs").read_text()
                if m:
                    brace = snip.find("{", m.start())
                    end = mp.find_matching(snip, brace, "{", "}")
                    body = snip[m.start() : end + 1]
                    try:
                        db = replace_in_db(db, name, body)
                    except KeyError:
                        ins = db.index("    pub async fn me_tails(")
                        db = db[:ins] + body + "\n\n" + db[ins:]
                    print("injected", name)
            elif name in ("approve_pilot", "me_tails"):
                m = re.search(rf"    pub async fn {name}\(", snip)
                if m:
                    brace = snip.find("{", m.start())
                    end = mp.find_matching(snip, brace, "{", "}")
                    db = replace_in_db(db, name, snip[m.start() : end + 1])
                    print("injected", name)

    header_extra = """
use subtle::ConstantTimeEq;

pub const SHARE_LIFETIME_AFTER_END_MS: i64 = 24 * 3600 * 1000;

#[derive(Debug, Clone)]
pub struct FlightShareRow {
    pub id: String,
    pub flight_id: String,
    pub club_id: String,
    pub created_by: String,
    pub token_hash: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FlightShareListItem {
    pub id: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked: bool,
}

#[derive(Clone, Debug)]
pub struct ResolvedShare {
    pub flight_id: String,
    pub club_id: String,
    pub tail: String,
    pub status: String,
}

"""
    if "FlightShareRow" not in db:
        db = db.replace(
            "    pub pilot_name: String,\n}\n\nimpl ClubDb {",
            "    pub pilot_name: String,\n}\n" + header_extra + "\nimpl ClubDb {",
            1,
        )
    if "new_share_token" not in db.split("\n")[0:15]:
        db = db.replace(
            "auth::{self, digest_token, new_device_token, new_id, SESSION_TTL_SEC}",
            "auth::{self, digest_token, new_device_token, new_id, new_share_token, SESSION_TTL_SEC}",
            1,
        )

    DB.write_text(db)
    print("wrote", DB)


if __name__ == "__main__":
    main()