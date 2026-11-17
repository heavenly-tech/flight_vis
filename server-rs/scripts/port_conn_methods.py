#!/usr/bin/env python3
"""Re-port only methods in db.rs that still contain conn."""
from __future__ import annotations

import importlib.util
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src/club/db_sqlite_merged.rs"
DB = ROOT / "src/club/db.rs"

import sys

sys.path.insert(0, str(ROOT / "scripts"))
from batch_port_methods import extract_method, port_method, replace_in_db  # noqa: E402
import method_port as mp  # noqa: E402

sqlite = SRC.read_text()
db = DB.read_text()
names = sorted(
    set(
        re.findall(
            r"\n    (?:pub )?(?:async )?fn (\w+)\([^)]*\)[^{]*\{[^}]*\bconn\.",
            db,
            flags=re.DOTALL,
        )
    )
)
# also find methods with params! or with_conn in body
for m in re.finditer(
    r"\n    ((?:pub )?(?:async )?fn (\w+)\([^)]*\)[^{]*\{)",
    db,
):
    start = m.start()
    brace = db.find("{", start)
    end = mp.find_matching(db, brace, "{", "}")
    body = db[start:end + 1]
    if "conn." in body or "params!" in body or "with_conn" in body:
        names.append(m.group(2))

names = sorted(set(names))
print("porting", len(names), "methods")
failed = []
for name in names:
    raw = extract_method(sqlite, name)
    if not raw:
        failed.append(name)
        continue
    try:
        body = port_method(raw)
        if "conn." in body or "params!" in body:
            print("skip still conn", name)
            continue
        db = replace_in_db(db, name, body)
        print("ok", name)
    except KeyError:
        print("missing", name)
    except Exception as e:
        print("err", name, e)

DB.write_text(db)
print("conn left", db.count("conn."), "failed", failed)