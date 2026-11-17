#!/usr/bin/env python3
"""Replace one method in db.rs with ported version from sqlite source."""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))
import method_port as mp

SRC = ROOT / "src/club/db_sqlite_source.rs"
DB = ROOT / "src/club/db_pg.rs"

def extract_method(src: str, name: str) -> str:
    m = re.search(rf"\n    pub fn {name}\(", src)
    if not m:
        m = re.search(rf"\n    fn {name}\(", src)
    if not m:
        raise SystemExit(f"method {name} not found in sqlite source")
    start = m.start() + 1
    brace = src.find("{", start)
    end = mp.find_matching(src, brace, "{", "}")
    return src[start : end + 1]

def replace_method(db: str, name: str, new_body: str, async_pub: bool = True) -> str:
    m = re.search(rf"\n    pub async fn {name}\(", db)
    if not m:
        m = re.search(rf"\n    async fn {name}\(", db)
    if not m:
        m = re.search(rf"\n    pub fn {name}\(", db)
    if not m:
        raise SystemExit(f"method {name} not in db.rs")
    start = m.start() + 1
    brace = db.find("{", start)
    end = mp.find_matching(db, brace, "{", "}")
    return db[:start] + new_body.rstrip() + db[end + 1 :]

def main() -> None:
    if len(sys.argv) < 2:
        raise SystemExit("usage: replace_method.py <name> [name...]")
    sqlite = SRC.read_text()
    db = DB.read_text()
    for name in sys.argv[1:]:
        raw = extract_method(sqlite, name)
        body = mp.port_body(raw)
        body = re.sub(r"\n    pub fn ", "\n    pub async fn ", body, count=1)
        body = re.sub(r"\n    fn ", "\n    async fn ", body, count=1)
        db = replace_method(db, name, body)
        print("replaced", name, "conn left", body.count("conn."))
    DB.write_text(db)

if __name__ == "__main__":
    main()