---
name: env
description: flight_vis environment variables. Use when configuring the API, Docker, or the Cesium token.
---

# Environment variables

Copy `.env.example` to `.env`. Do not commit `.env`.

| Variable | Used by | Default | Meaning |
| --- | --- | --- | --- |
| `FLIGHT_VIS_PASSWORD` | API | empty | Login password. Empty locks uploads, saves, renames, deletes, and video writes. Public slug reads still work. Trimmed. |
| `FLIGHT_DATA` | API | `data` | Directory of flight folders (`meta.json`, `track`, video files). Relative paths are from the process cwd. |
| `FLIGHT_VIS_DIST` | API | `dist` | Built frontend. Canonicalized when the directory exists. |
| `PORT` | API | `8080` | Listen port. Non-numeric values fall back to 8080. |
| `TRUSTED_PROXY_CIDRS` | API | `172.18.0.0/16,127.0.0.1/32,::1/128` | Comma-separated IPv4/IPv6 CIDRs whose socket peers may set the login client IP via `X-Real-IP` / `X-Forwarded-For`. Missing, empty, or unparseable values fall back to the default. |
| `VITE_CESIUM_ION_TOKEN` | Vite | unset | Optional Cesium ion token for the globe. |

Docker sets `PORT`, `FLIGHT_DATA=/data`, and `FLIGHT_VIS_DIST=/app/dist`. Compose also sets `FLIGHT_VIS_PASSWORD` from the host and the proxy hostnames `VIRTUAL_HOST`, `LETSENCRYPT_HOST`, and `VIRTUAL_PORT` (those three are for the reverse proxy, not read by the binary).

There is no `NODE_ENV`. The Rust binary does not read `CARGO_MANIFEST_DIR` for data, dist, or the login page. Those paths come only from the environment variables above, or the cwd-relative defaults. Sample seeding still looks up the compile-time sample GPX paths.
