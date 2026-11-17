#!/usr/bin/env python3
"""Add .await for async ClubDb calls in routes, share_routes, checkin_landing."""
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FILES = [
    ROOT / "src/club/routes.rs",
    ROOT / "src/club/share_routes.rs",
    ROOT / "src/club/checkin_landing.rs",
]

ASYNC_DB_METHODS = [
    "default_club_id",
    "register",
    "login",
    "logout",
    "viewer_from_session",
    "viewer_with_pilot",
    "pilot_from_session",
    "club_default_locale",
    "set_pilot_locale",
    "set_club_default_locale",
    "me_tails",
    "resolve_device_token",
    "start_flight",
    "patch_flight",
    "end_flight",
    "club_map_for_viewer",
    "public_map",
    "admin_map",
    "admin_map_tails",
    "set_admin_map_tails",
    "flight_visible_to",
    "fixes_for_flight",
    "list_my_flights",
    "delete_my_flight",
    "list_types",
    "upsert_type",
    "list_aircraft",
    "upsert_aircraft",
    "list_all_pilots",
    "list_pending_pilots",
    "approve_pilot_with_checkout",
    "list_checkouts",
    "add_checkout",
    "list_pilot_access_requests",
    "create_access_request",
    "admin_pending_counts",
    "list_admin_access_requests",
    "approve_access_request",
    "reject_access_request",
    "register_device_for_install",
    "create_device",
    "revoke_device_by_id",
    "revoke_device_by_install",
    "list_my_devices",
    "resolve_display_session",
    "submit_checkin",
    "mint_checkin_qr_token",
    "display_map",
    "pilot_club_id",
    "club_slug",
    "create_display_session",
    "list_display_sessions",
    "revoke_display_session",
    "club_id_for_slug",
    "ensure_kiosk_qr_defaults",
    "kiosk_qr_layout",
    "set_kiosk_qr_layout",
    "reset_kiosk_qr_layout",
    "device_pair_token_for_pilot",
    "device_pilot_id",
    "create_flight_share",
    "list_flight_shares",
    "revoke_flight_share",
    "resolve_share_token",
    "resolve_club_context",
    "get_flight",
    "approve_pilot",
    "upsert_aircraft",
    "issue_open_checkin",
    "consume_checkin",
]

VIEWER_FN = re.compile(
    r"fn viewer\(state: &AppState, headers: &HeaderMap\) -> Viewer"
)
VIEWER_CALL = re.compile(r"viewer\((&state[^)]*)\)")


def add_await_db(text: str) -> str:
    for method in ASYNC_DB_METHODS:
        pat = re.compile(
            rf"(\bdb\.{re.escape(method)}\([^;]*?\))(?!\s*\.await)"
        )

        def repl(m: re.Match[str]) -> str:
            s = m.group(1)
            if s.rstrip().endswith(".await"):
                return s
            return s + ".await"

        text = pat.sub(repl, text)
    return text


def fix_viewer_fn(text: str) -> str:
    text = VIEWER_FN.sub(
        "async fn viewer(state: &AppState, headers: &HeaderMap) -> Viewer", text
    )
    text = VIEWER_FN.sub(
        "async fn viewer(state: &AppState, headers: &HeaderMap) -> Viewer", text
    )
    text = text.replace(
        """    match state.club.as_ref() {
        Some(db) => db.viewer_from_session(session.as_deref(), now).await,
        None => Viewer::Anonymous,
    }""",
        """    match state.club.as_ref() {
        Some(db) => db.viewer_from_session(session.as_deref(), now).await,
        None => Viewer::Anonymous,
    }""",
    )
    # viewer_from_session in viewer()
    text = re.sub(
        r"db\.viewer_from_session\(session\.as_deref\(\), now\)(?!\s*\.await)",
        "db.viewer_from_session(session.as_deref(), now).await",
        text,
    )
    text = VIEWER_CALL.sub(r"viewer(\1).await", text)
    # avoid double await
    text = text.replace(".await.await", ".await")
    return text


def main() -> None:
    for path in FILES:
        raw = path.read_text()
        out = fix_viewer_fn(add_await_db(raw))
        path.write_text(out)
        print("updated", path.name)


if __name__ == "__main__":
    main()