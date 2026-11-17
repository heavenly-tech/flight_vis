#!/usr/bin/env bash
# Decode owner-provided *.b64.txt uploads into assets/brand/reference/.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/assets/brand/reference"
mkdir -p "$OUT"
shopt -s nullglob
for f in "$ROOT"/uploads/*.b64.txt /uploads/*.b64.txt; do
  base="$(basename "$f" .b64.txt)"
  out="$OUT/$base"
  echo "decode $f -> $out"
  base64 -d <"$f" >"$out"
done