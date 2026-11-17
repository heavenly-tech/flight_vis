#!/usr/bin/env python3
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1] / "src/club"
FILES = [
    "db.rs",
    "db_checkin.rs",
    "db_devices.rs",
    "db_map_view.rs",
    "db_kiosk_layout.rs",
    "db_shares.rs",
    "db_access.rs",
    "club_scope.rs",
    "mail_suppress.rs",
]


def extract_impl_methods(path: Path) -> str:
    text = path.read_text()
    if "impl ClubDb" not in text:
        return ""
    impl = text[text.index("impl ClubDb {") + len("impl ClubDb {") :]
    mod_tests = re.search(r"\n#\[cfg\(test\)\]\nmod tests\b", impl)
    if mod_tests:
        impl = impl[: mod_tests.start()]
    # strip trailing closing brace of impl
    depth = 0
    end = None
    for i, c in enumerate(impl):
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth < 0:
                end = i
                break
    if end is None:
        end = impl.rfind("\n}")
    return impl[:end].strip() + "\n"


def main() -> None:
    base = ROOT / "db.rs"
    raw = base.read_text()
    impl_start = raw.index("impl ClubDb {")
    tail = raw[raw.index("\npub fn now_ms") :]
    header = raw[: impl_start + len("impl ClubDb {")]

    merged = []
    skip = {"open", "open_in_memory", "with_conn", "with_conn_any", "migrate"}
    seen = set()
    for name in FILES:
        p = ROOT / name
        if not p.exists():
            continue
        block = extract_impl_methods(p)
        for line in block.splitlines():
            m = line.strip()
            if m.startswith("pub fn ") or m.startswith("fn ") or m.startswith("pub(crate) fn "):
                fn = m.split("(")[0].split()[-1]
                if fn in skip:
                    continue
                if fn in seen and name != "db.rs":
                    continue
                seen.add(fn)
        merged.append(f"    // from {name}\n{block}")

    out = header + "\n" + "\n".join(merged) + "\n}\n\n" + tail
    (ROOT / "db_sqlite_merged.rs").write_text(out)
    print("wrote merged", len(out.splitlines()), "methods", len(seen))


if __name__ == "__main__":
    main()