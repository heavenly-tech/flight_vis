#!/usr/bin/env bash
# Post-migrate smoke: public demo slug and legacy redirect (no session cookie).
set -euo pipefail
BASE="${FLIGHT_CLUB_PUBLIC_URL:-https://flight.heavenly.cl}"

echo "GET ${BASE}/demo"
curl -fsS -o /dev/null -w "demo HTTP %{http_code}\n" "${BASE}/demo"

echo "GET ${BASE}/api/flights/demo"
curl -fsS -o /dev/null -w "api demo HTTP %{http_code}\n" "${BASE}/api/flights/demo"

echo "HEAD ${BASE}/vuelo-5 -> Location"
curl -fsSI "${BASE}/vuelo-5" | tee /dev/stderr | grep -qi '^location:.*/demo'