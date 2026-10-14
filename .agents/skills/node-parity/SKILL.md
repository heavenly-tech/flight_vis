---
name: node-parity
description: Node server parity contract for the Rust API. Use when changing auth, slugs, flight JSON, static files, or error bodies.
---

# Node parity contract

The Rust server replaced the Node server. Keep byte-level behavior below unless a review explicitly changes it. Tests in `server-rs/src` lock the contract in.

## Auth

- Cookie name `fv_session`.
- Token `v1.{exp}.{hmac-sha256-hex}`.
- Key is SHA-256 of the bytes `flight-vis-login-v1` concatenated with the password. HMAC-SHA256 covers the ASCII payload `v1.{exp}`.
- TTL is 7 days. Cookie flags: `Path=/; HttpOnly; SameSite=Lax`, plus `Secure` when `X-Forwarded-Proto` is `https`.
- Password check compares SHA-256 hex digests in constant time.
- Vector: password `secret`, `now=1700000000` mints `v1.1700604800.c8dfb2b0d238c9e31dfe63622d8cd6975c0bc3be1c3052a7525f4adea25774ee`.
- `next` must be a single-slash path, must not contain `\` or `://`, and must not be `/login`. Anything else becomes `/`.
- JSON clients send `Accept: application/json` or `Content-Type: application/json`. Others get redirects.
- Missing password: `401 {"error":"locked"}`. Bad password: `401 {"error":"unauthorized"}`.

## Slugs and flights

- Normalize with NFKD, drop combining marks, lowercase, map other characters to a single `-`, trim dashes, cap at 64 characters.
- Reject empty slugs, leading or trailing `-`, `--`, uppercase, and the reserved names `api`, `assets`, `cesiumstatic`, `favicon.ico`, `health`, `login`, `logout`, `samples`, `static`.
- On-disk layout: `{FLIGHT_DATA}/flights/{slug}/meta.json` and `track`. Writes are temp file plus rename, under one process-wide mutex.
- Public flight JSON omits `videos` while that array is empty. Field order for a flight without videos is `slug`, `name`, `fileName`, `trimStartMs`, `trimEndMs`, `tags`, `updatedAt`, `createdAt`, `track`.
- Library list is newest `updatedAt` first.
- `GET /api/flights/:slug` is public. List, upload, save, rename, and delete need a session.
- Invalid slug on flight routes is `400 {"error":"Invalid slug"}`.
- Rename onto an existing slug without `overwrite` is `409 {"error":"exists"}`. Upload `conflict=error` is `409` with a `slugs` array.
- `PUT` with malformed JSON is `400 {"error":"Invalid JSON"}`. Node threw and surfaced a 500; do not revert this to 500.

## Static files

- Drop empty and `.` segments. Any `..` segment is refused. The joined path must stay under the dist directory.
- Unknown files fall back to `index.html` (`no-cache`). Missing `index.html` is 500.
- Hashed assets match `/-[A-Za-z0-9_-]{6,}\.[a-z0-9]+$/` and use `public, max-age=31536000, immutable`. Everything else is `no-cache`.

## Intentional differences

- Flight `PUT` / `POST` / `PATCH` accept bodies up to 64 MB (`DefaultBodyLimit` on the flight router in `build_router`). Other `Bytes` extractors keep Axum's 2 MB default.
- `POST /api/flights/:slug/videos` with an invalid slug is `400 {"error":"Invalid slug"}`. A valid slug that is not on disk is still `404 {"error":"Not found"}`.
- Video ids are at most 32 characters. The body is `[0-9a-z-]`. New ids are `vid-` plus 10 base-36 characters. Ids already stored as `vid_` plus that same body still pass. Any other underscore, slash, dot, or uppercase character is `400 {"error":"Invalid video id"}`.
- Failing to create the upload file is logged server-side and returned as `500 {"error":"could not store video"}`. Do not put the OS error in the JSON body.
- The transcode queue is a bounded channel of 32 jobs. ffmpeg is invoked with `-protocol_whitelist file` before `-i`.
- `POST /login` allows 8 attempts per client address per 60 seconds, then `429 {"error":"too many attempts"}`. The address is the first `X-Forwarded-For` hop, else `X-Real-IP`, else the socket address.
