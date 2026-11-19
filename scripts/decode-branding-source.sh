#!/usr/bin/env bash
# Decode branding-source.tar.gz.b64.txt into branding/source/ (owner bundle).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/branding/source"
mkdir -p "$SRC"
for f in "$ROOT"/uploads/branding-source.tar.gz.b64.txt "$ROOT"/branding-source.tar.gz.b64.txt; do
  if [[ -f "$f" ]]; then
    tmp="$(mktemp)"
    base64 -d <"$f" >"$tmp"
    tar xzf "$tmp" -C "$SRC"
    rm -f "$tmp"
    echo "extracted branding source into $SRC"
    exit 0
  fi
done
echo "no branding-source.tar.gz.b64.txt found" >&2
exit 1