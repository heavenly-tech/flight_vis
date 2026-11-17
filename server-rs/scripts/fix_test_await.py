#!/usr/bin/env python3
"""Insert .await before common db test chains."""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
paths = list(ROOT.glob("src/club/**/*.rs"))

METHODS = r"default_club_id|register|login|approve_pilot|add_checkout|create_device|issue_open_checkin|start_flight|patch_flight|end_flight|append_fix|append_fixes|upsert_type|upsert_aircraft|mint_checkin_qr_token|submit_checkin|club_checkin_settings|flight_visible_to|set_admin_map_tails|display_map|seed_demo_if_empty|consume_checkin|create_display_session|list_types|add_checkout_typed|approve_pilot_with_checkout|create_access_request|reject_access_request|approve_access_request|auto_end_idle|logout|set_club_default_locale|set_pilot_locale|club_default_locale|me_tails|delete_my_flight"

pat = re.compile(
    rf"(\bdb\.(?:{METHODS})\([^)]*\))(\s*\.(unwrap|expect|is_ok|is_err|map_err|unwrap_or|map)\b)"
)

def fix(text: str) -> str:
    prev = None
    while prev != text:
        prev = text
        text = pat.sub(r"\1.await\2", text)
    text = text.replace(".await.await", ".await")
    return text

for path in paths:
    raw = path.read_text()
    out = fix(raw)
    if out != raw:
        path.write_text(out)
        print("fixed", path.relative_to(ROOT))