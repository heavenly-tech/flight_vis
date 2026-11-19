#!/usr/bin/env bash
# Decode owner-provided *.b64.txt uploads into branding/source/.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/branding/source"
mkdir -p "$OUT"
shopt -s nullglob
for f in "$ROOT"/uploads/*.b64.txt /uploads/*.b64.txt; do
  base="$(basename "$f" .b64.txt)"
  out="$OUT/$base"
  echo "decode $f -> $out"
  base64 -d <"$f" >"$out"
done