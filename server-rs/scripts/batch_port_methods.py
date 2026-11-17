#!/usr/bin/env python3
"""Re-port methods in db.rs from db_sqlite_merged using method_port + full_pg_convert."""
from __future__ import annotations

import importlib.util
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src/club/db_sqlite_merged.rs"
DB = ROOT / "src/club/db.rs"

spec = importlib.util.spec_from_file_location("mp", ROOT / "scripts/method_port.py")
mp = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mp)

spec2 = importlib.util.spec_from_file_location("fpc", ROOT / "scripts/full_pg_convert.py")
fpc = importlib.util.module_from_spec(spec2)
spec2.loader.exec_module(fpc)


def extract_method(src: str, name: str) -> str | None:
    for pat in (rf"\n    pub fn {name}\(", rf"\n    fn {name}\(", rf"\n    pub\(crate\) fn {name}\("):
        m = re.search(pat, src)
        if m:
            start = m.start() + 1
            brace = src.find("{", start)
            end = mp.find_matching(src, brace, "{", "}")
            return src[start : end + 1]
    return None


def port_method(raw: str) -> str:
    body = mp.port_body(raw)
    body = fpc.convert_query_row_optional(body)
    body = fpc.convert_query_row_tuple(body)
    body = fpc.convert_prepare_query_map(body)
    body = body.replace("Self::checkout_ok(conn,", "Self::checkout_ok(&self.pool,")
    body = body.replace(
        "Self::checkout_ok(&self.pool,",
        "Self::checkout_ok(&self.pool,",
    )
    body = re.sub(r"\n    pub fn ", "\n    pub async fn ", body, count=1)
    body = re.sub(r"\n    fn ", "\n    async fn ", body, count=1)
    body = re.sub(r"\n    pub\(crate\) fn ", "\n    pub(crate) async fn ", body, count=1)
    body = body.replace("Self::checkout_ok(conn,", "Self::checkout_ok(pool,")
    return body


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
            return db[:start] + new_body.rstrip() + db[end + 1 :]
    raise KeyError(name)


NAMES = [
    "approve_pilot",
    "checkout_ok",
    "me_tails",
    "start_flight",
    "start_flight_atomic",
    "patch_flight",
    "end_flight",
    "device_pilot_id",
    "resolve_device_token",
    "open_flight_for_device",
    "append_fix",
    "append_fixes",
    "auto_end_idle",
    "get_flight",
    "fixes_for_flight",
    "club_map_for_viewer",
    "public_map",
    "admin_map",
    "admin_map_tails",
    "set_admin_map_tails",
    "create_device",
    "device_pair_token_for_pilot",
    "list_my_devices",
    "list_my_flights",
    "delete_my_flight",
    "upsert_type",
    "list_types",
    "list_aircraft",
    "list_all_pilots",
    "list_pending_pilots",
    "list_checkouts",
    "add_checkout",
    "pilot_club_id",
    "club_checkin_settings",
    "checkin_replay",
    "insert_checkin_row",
    "latest_device_fixes",
    "resolve_display_session",
    "list_display_sessions",
    "seed_demo_if_empty",
    "club_checkin_window_hours",
    "set_club_checkin_window_hours",
    "admin_set_club_checkin_window",
    "has_open_checkin",
    "latest_open_checkin",
    "admin_extend_pilot_checkin",
    "count_devices_for_pilot",
    "register_device_for_install",
    "end_active_flights_for_device",
    "revoke_device_row",
    "revoke_device_by_install",
    "club_id_for_slug",
    "club_slug",
    "membership_in_club",
    "ensure_membership_row",
    "record_audit",
    "pilot_has_open_checkin",
    "count_pending_access_requests",
    "create_access_request",
    "list_pilot_access_requests",
    "list_admin_access_requests",
    "approve_access_request",
    "reject_access_request",
]


def main() -> None:
    sqlite = SRC.read_text()
    # merge access methods into sqlite text for lookup
    acc = (ROOT / "src/club/db_access.rs").read_text()
    if "rusqlite" in acc:
        sqlite = sqlite.replace(
            "\n}\n\npub fn now_ms",
            "\n" + "\n".join(
                re.findall(r"impl ClubDb \{([\s\S]*?)\n\}", acc)
            )
            + "\n}\n\npub fn now_ms",
            1,
        )
    db = DB.read_text()
    failed = []
    for name in NAMES:
        raw = extract_method(sqlite, name)
        if not raw:
            failed.append(name)
            continue
        try:
            body = port_method(raw)
            if "conn." in body or "params!" in body:
                print("skip", name, "conn", body.count("conn."))
                continue
            db = replace_in_db(db, name, body)
            print("ok", name)
        except KeyError:
            print("missing in db", name)
        except Exception as e:
            print("err", name, e)
    DB.write_text(db)
    print("conn left", db.count("conn."), "failed extract", failed)


if __name__ == "__main__":
    main()