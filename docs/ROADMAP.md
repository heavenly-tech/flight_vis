# Roadmap: live flight paths

Goal: a pilot can stream their position from a phone or logger, watch their own
flight on the globe as it happens, and later share a live view with chosen
people — without adding a sidecar service or weakening the current privacy
model.

Current state: **there is no live ingest, no stream endpoint, and no live
storage today.** This document describes what exists, then two phases that add
it.

---

## 1. Context

### Server

- One Axum 0.8 crate (`server-rs`), binary `flight_vis`, Tokio runtime. No
  database, no queue, no external process except `ffmpeg` for video transcodes.
- Routes today: `GET /api/health`, `GET /api/auth/me`, `GET|POST /login`,
  `GET|POST /logout`, flight CRUD under `/api/flights`, video
  upload/transcode/serve under `/api/flights/{slug}/videos`, plus an SPA
  fallback that serves `index.html` for unknown paths.
- `AppState` carries the dist dir, `Auth`, the disk `Store`, a bounded video
  job sender, and the in-memory login throttle. New long-lived state (sessions,
  subscribers, rate buckets) belongs in the same struct.
- Background work today is a single video transcode worker plus a requeue on
  boot. There is no scheduler; a live retention job would be the second one.

### Storage

- `{FLIGHT_DATA}/flights/{slug}/meta.json` and `.../track`. `track` is the
  original IGC/GPX text; the viewer parses it in the browser.
- `meta.json`: `slug`, `name`, `fileName`, `trimStartMs`, `trimEndMs`, `tags`,
  `videos`, `updatedAt`, `createdAt`. Pretty-printed JSON, field order locked
  by tests.
- All writes go through one process-wide async mutex and temp-file + rename.
  Library order is `updatedAt` descending. Slugs: NFKD, lowercase, ASCII
  dashes, max 64 chars, reserved names (`api`, `login`, `assets`, …).
- Public reads: `GET /api/flights/{slug}` and the `/{slug}` page. Every write
  needs a session.

### Auth

- One shared password (`FLIGHT_VIS_PASSWORD`). Missing password = locked mode:
  public pages still render, writes return `401 {"error":"locked"}`.
- Stateless cookie `fv_session`, value `v1.{exp}.{hmac-sha256-hex}`, 7-day TTL,
  `HttpOnly; SameSite=Lax`, `Secure` when `X-Forwarded-Proto: https`.
- Password and signature checks use constant-time comparison (`subtle`).
- Login throttle: 8 attempts / 60 s per resolved client IP; forwarding headers
  are trusted only from `TRUSTED_PROXY_CIDRS` (default Docker proxy network +
  loopback). The same resolver is reusable for ingest and share throttles.

### Frontend

- Vite 7 + TypeScript + CesiumJS. `src/main.ts` boots the viewer, wires
  controls, and loads a slug from the path. `src/api.ts` holds all fetches.
  `src/library.ts` is the saved-track panel.
- `src/track/*` parses GPX/IGC, derives speed/vario/distance/heading
  (`enrich.ts`), handles trim and types. `src/viz/viewer.ts` owns Cesium
  primitives, the follow camera and on-demand rendering.
- `viewer.setFlight()` rebuilds the whole path (positions, colors, entity,
  clock). There is no append path and no incremental track model yet.
- URL parsing accepts a single path segment only (`slugFromPath`), so a new
  route shape is needed for live views.

### Deployment and PR #9

- CI runs `cargo test`, `cargo clippy -D warnings`, `npm run build` and
  `docker build`.
- **PR #9 (open, not merged)** changes the runtime stage to `FROM scratch`:
  a static musl binary, the `ffmpeg`/`ffprobe` shared-library closure, CA certs,
  no shell, no package manager, no `wget`/`curl`, uid `10001`, and a
  healthcheck implemented as `/app/flight_vis healthcheck` (loopback HTTP probe
  done inside the binary).
- Consequence for this roadmap: no cron, no shell scripts, no sidecars, no
  runtime library installs. Everything must run inside the one binary and write
  only to `/data`. New Rust dependencies must be pure Rust or compile their C
  into the binary (SQLite via a bundled feature is acceptable but adds build
  time; plain Rust crates are preferred).

### Design stance

Store live data as **append-only files on the `/data` volume**, next to the
existing flight store, with an in-memory ring buffer for hot reads. Volume is
small (one fix per second per device is ~2.8 MB per six-hour flight), and the
existing atomic-write, single-mutex pattern already fits. Bundled SQLite is the
escape hatch if multi-user queries arrive; it is not needed for Phases 1–2.

---

## 2. Phase 1 — your own live flight path

### 2.1 Ingest sources and protocol

| Candidate | Fit | Notes |
| --- | --- | --- |
| Phone app / GPS logger HTTP push | **Primary** | Any client that can POST JSON. Batches absorb tunnel/valley gaps. |
| OsmAnd / Traccar "osmand" HTTP protocol | Compat adapter | Form/query push already implemented by several tracker apps; maps to the same internal fix. |
| OGN / FLARM via APRS-IS | Later, read-only | Third-party TCP feed, aircraft-ID mapping, and public-by-nature data. Not the first source. |

**Decision:** one canonical endpoint, `POST /api/ingest`, JSON body, plus a thin
compat adapter `GET /api/ingest/osmand` for clients that cannot set headers.
Both normalize into the same internal fix type, so an OGN adapter later plugs in
without touching storage or streaming.

Why HTTP push first: TLS and reverse proxying already exist, batches are easy to
retry, and no inbound port beyond 443 is required. APRS-IS would add a
long-lived TCP client and an identity-mapping problem that is better solved
after the pipeline is proven.

Request shape:

```http
POST /api/ingest HTTP/1.1
Authorization: Bearer fvd_<64 hex chars>
Content-Type: application/json

{ "device": "phone", "fixes": [
  { "t": 1750000000123, "lat": -32.9001, "lon": -71.2002,
    "alt": 1203.4, "spd": 21.4, "trk": 187.2, "acc": 8.5 }
] }
```

**Device tokens.** `fvd_` + 32 random bytes (hex). Only the SHA-256 hash and a
short display prefix are stored, under `{FLIGHT_DATA}/devices/{deviceId}.json`;
the plaintext is shown once at creation. Tokens are per device, listed and
revoked by the owner, and compared in constant time. The compat adapter accepts
`?token=` (those clients cannot send headers) and therefore takes one fix per
request, since URLs are bounded well below the JSON body limit; that token
variant gets a shorter TTL and its value is redacted from request logs.

**Limits and validation.**

| Limit | Value (default, env-tunable) |
| --- | --- |
| Body size | 256 KiB (ingest router only; the 64 MB limit stays on flight routes) |
| Fixes per request | 600 |
| Requests per token | 2/s, burst 10 |
| Requests per IP | 30/s |
| Fixes per device per day | 200 000 |
| Concurrent devices | 16 |

Validation: latitude `[-90, 90]`, longitude `[-180, 180]`, altitude
`[-1000, 20000]` m, speed `[0, 150]` m/s, track `[0, 360)`, `t` within
[session start − 1 h, now + 60 s], `acc ≤ 1000` m. Fixes must be strictly newer
than the device's last accepted fix; duplicates are dropped by `(device, t)`.
Rejections are counted and never echo the token. `429` responses carry
`Retry-After`.

### 2.2 Position storage

```
{FLIGHT_DATA}/
  flights/{slug}/…                 # unchanged, finished tracks
  live/{sessionId}/
    session.json                   # owner, name, state, visibility, origin, promotedSlug
    fixes.ndjson                   # append-only, one fix per line
    fixes.idx                      # u64 LE offset every 256 fixes
  devices/{deviceId}.json          # token hash, label, createdAt, revokedAt
  audit/YYYY-MM.jsonl              # append-only (Phase 2 writes here)
```

- `sessionId` is `live-` + 12 random base36 characters, never derived from the
  flight name, so private ids cannot be enumerated.
- Append path: one writer task owns each open file; requests enqueue fixes over
  a bounded channel. Batches are written with a single append, and `sync_data`
  runs every 5 s and on close. A crash loses at most the last few seconds.
- `fixes.idx` gives O(1) seeks for backfill and for the promote step. At 1 Hz a
  six-hour flight is ~21 600 records and under 1 KB of index.
- Hot state: `HashMap<SessionId, Arc<LiveSession>>` in `AppState` behind an
  `RwLock`, each with a ring of the last 3 600 fixes, the last accepted
  timestamp per device, and a `tokio::sync::broadcast` sender.
- `session.json` is rewritten with the existing atomic temp+rename helper on
  state changes (start, idle, live, finished), not per fix.

**Retention.**

| Data | Default | Behaviour |
| --- | --- | --- |
| Ring buffer | 1 h | Overwritten continuously |
| Raw fixes, promoted session | 30 days | Pruned by a daily task started at boot |
| Raw fixes, never promoted | 7 days | Auto-finish, then prune |
| `flights/{slug}` | Forever | Existing lifecycle only |
| Audit log | 90 days | Rotated monthly, pruned daily |

The prune task is a plain `tokio::time::interval` loop started from `main` next
to the video worker. No cron, no shell.

**Promotion to a normal stored track.** `POST /api/live/{id}/finish` (or the
idle auto-finish after 20 min without fixes) converts the fix log to GPX 1.1
text (`<trkpt lat lon><ele><time>`), then calls the existing
`Store::write_flight` with `name`, `fileName: live-YYYYMMDD-hhmm.gpx`,
`trimStartMs/EndMs: null`, `tags: []`. The result is an ordinary slug served by
the current public read route and opened by the current viewer. Allocation uses
`allocate_slug`; `session.json` records `promotedSlug` so a repeated finish is
idempotent. Fewer than two fixes returns `409`. Raw fixes are retained after
promotion until the retention window closes, so the original can be re-promoted.
The promoted slug is public under the current model, like any stored track, so
the finish UI must say so explicitly; live sessions themselves stay owner-only
until a slug is created.

### 2.3 Live updates: SSE, not WebSocket

**Decision: Server-Sent Events.**

- The data flow is one-way after ingest. Every client action is an ordinary POST
  (create, finish, revoke), so WebSocket bidirectionality buys nothing.
- The deployment fronts the app with an HTTP reverse proxy. SSE is a plain
  response; WebSocket adds upgrade headers and proxy configuration that cannot
  be validated from this repository.
- `EventSource` reconnects automatically, and `Last-Event-ID` is a native
  browser behaviour. With WebSocket we would reimplement reconnect, resume,
  heartbeats and backoff in TypeScript.
- Streams are testable with `curl -N` and with `tower::ServiceExt` in Rust
  tests; a WebSocket test needs a client upgrade handshake.
- Axum already provides streaming response bodies (`ReaderStream` is used for
  static files); SSE needs no new dependency.

Trade-off accepted: no binary frames and a per-origin connection cap on
HTTP/1.1. One stream per watched session is far below six, and payloads are
tiny JSON.

Endpoint and contract:

```
GET /api/live/{sessionId}/stream      (session cookie, or share cookie in Phase 2)
Accept: text/event-stream
Last-Event-ID: 1841

retry: 3000

event: hello
data: {"session":"live-…","name":"…","state":"live","delayS":0}

event: snapshot
data: {"reset":true,"fixes":[…up to 300 latest points…]}

id: 1842
event: fix
data: {"t":1750000000123,"lat":-32.9001,"lon":-71.2002,"alt":1203.4,"spd":21.4,"trk":187.2}
```

- `id` is a per-session monotonic sequence, assigned at write time, so it is
  stable across reconnects and matches the on-disk order.
- On join with no `Last-Event-ID`: send `hello`, then `snapshot` (latest 300
  fixes, enough to draw the recent trail), then live `fix` events.
- With `Last-Event-ID = n`: replay `n+1 …` from the ring. If `n` is older than
  the ring's oldest sequence, send `snapshot` with `reset: true` and continue
  ids from the newest sequence; the client discards its trail and redraws.
- Keepalive: a `: ping` comment every 15 s, below typical 60 s proxy idle
  timeouts and harmless to `Last-Event-ID`.
- Session end: `event: end` with a reason (`finished`, `deleted`, `slow`).
- Backpressure: each subscriber gets a bounded channel (256 events). A full
  channel drops the subscriber with `end: slow`; `EventSource` reconnects and
  resyncs from a snapshot. No unbounded buffering, no per-client tasks beyond
  the response stream.
- Access: 404 for unknown or unauthorized sessions (do not confirm existence);
  401 only when no session cookie is present at all. Responses set
  `Cache-Control: no-store`, `X-Accel-Buffering: no`.
- Caps: 8 concurrent streams per session, 32 per client IP, 256 total.

### 2.4 Cesium frontend changes

- New route `/live/{sessionId}`. Unknown paths already fall back to
  `index.html`, so only a path parser and a branch in `main.ts` are needed
  (`src/track/slug.ts` gains `liveIdFromPath`; `slugFromPath` keeps its
  single-segment rule).
- New module `src/live.ts`: `EventSource` wrapper, reconnect/backoff display
  state, and a live reducer that appends fixes into the existing `TrackPoint`
  shape. Reuse `haversineMeters` and `headingRadians` from `src/track/geo.ts`
  so speed, vario, distance and heading match stored-track playback.
  `enrichFlight` is not reused per fix: it sorts and smooths the whole array.
- New viewer methods: `appendTrack(points)` appends positions/colors and extends
  the last primitive chunk without `rebuildTrack`; `setLive(true)` hides
  trim/tag affordances, extends the clock stop time as fixes arrive, and keeps
  `requestRenderMode` explicit. `resetTrack()` handles a snapshot with
  `reset: true`.
- Follow camera: fixes arrive at ~1 Hz; interpolate between the last two fixes
  and feed `setSample` at the existing frame cadence (~10 Hz) so motion is
  smooth without new camera code. Dead reckoning stops after 5 s without a fix;
  the marker then holds and the UI marks the feed stale.
- UI: a `LIVE` pill next to the flight name with last-fix age. `< 10 s` normal,
  `10–60 s` amber, `> 60 s` "signal lost" plus a reconnect hint. Reuse
  `#status` for transient errors. HUD keeps working unchanged.
- Trim, tag and replay controls are disabled in live mode; "Save as track"
  calls the finish endpoint and navigates to the promoted slug.
- Performance: cap rendered live points by thinning to 3 s spacing once a
  session exceeds ~5 000 points; storage stays at full resolution.

---

## 3. Phase 2 — shareable live paths

### 3.1 Identity reality

There is one shared password, so every logged-in browser is the same principal.
Phase 2 therefore uses **capability links**, not accounts: a share is a bearer
token that grants read access to specific sessions. "Other app users" are
addressed by handing each person their own link, which also makes per-person
revocation and auditing possible without a user table. If per-person logins are
wanted later, that is a separate change (see open questions).

### 3.2 Share tokens

- 32 random bytes from the OS CSPRNG (`getrandom`), formatted
  `fvs_` + 64 hex characters. High entropy means a plain SHA-256 hash at rest
  is sufficient; no password KDF is needed for random tokens.
- At rest, only `sha256(token)` and a 12-character display prefix are stored in
  `{FLIGHT_DATA}/shares/{shareId}.json`. The plaintext is returned once, at
  creation.
- Comparison uses `subtle::ConstantTimeEq` on equal-length digests, and the
  lookup is by hash, so a wrong token cannot be distinguished from a missing
  one by timing or status (both 404).
- Grant record: `{ id, label, tokenHash, tokenPrefix, sessions[], expiresAt,
  createdAt, revokedAt, maxConcurrent }`.
- Expiry: default 24 h; allowed range 1 h–7 days. Expired and revoked grants
  fail immediately on the next request; no cache to invalidate.
- Per-viewer allowlists are the grants themselves: one link per person or
  group, each with its own label, session list, expiry and concurrency cap. A
  single "team" link is explicitly discouraged in the UI because it cannot be
  revoked per person.
- Link format: `https://example.com/watch#t=<token>`. The fragment never
  reaches the server or proxy logs. The page redeems it with
  `POST /api/shares/redeem`, which sets an `HttpOnly`, `SameSite=Lax` cookie
  scoped to `/api/live/` and then removes the fragment. This keeps
  `EventSource` (which cannot send `Authorization` headers) working, and the
  plaintext token is never used as a query parameter.
- `POST /api/shares`, `GET /api/shares`, `POST /api/shares/{id}/revoke` require
  the owner session. Creation is capped at 30/h.

### 3.3 Privacy defaults and controls

- **Private by default.** A new session is visible to the owner session only.
  Publishing is an explicit toggle per session, and the UI states clearly that
  a published stream is reachable by anyone holding a valid link.
- **Delay.** `delayS ∈ {0, 300, 900, 3600}` applies server-side to snapshot and
  fix events alike: only fixes with `t ≤ now − delay` are emitted. The owner
  preview is not delayed.
- **Origin and landing suppression.** On session start the server records the
  first fix as `origin`. Published data omits fixes within `originRadiusM`
  (default 2 000 m) for the first `originDelayS` (default 600 s). On finish,
  the published trail is trimmed by the same radius around the final fix. Both
  are server-side filters, so a modified client cannot bypass them.
- Never shared: device tokens, raw fix files, IP addresses, audit records, and
  unpublished sessions. Watching pages send `X-Robots-Tag: noindex`,
  `Referrer-Policy: no-referrer`, and are excluded from the share link preview
  (no OpenGraph tags).
- Deletion: finishing or deleting a session invalidates its grants and sends
  `event: end` to open streams.

### 3.4 Audit trail

Append-only `{FLIGHT_DATA}/audit/YYYY-MM.jsonl`:

| Event | Fields |
| --- | --- |
| `device.create` / `device.revoke` | ts, deviceId, tokenPrefix, actor |
| `share.create` / `share.revoke` | ts, shareId, label, tokenPrefix, sessions, expiresAt |
| `share.redeem` | ts, shareId, outcome, coarse IP |
| `stream.open` / `stream.close` | ts, sessionId, shareId or owner, coarse IP, fixes sent, duration |
| `session.publish` / `session.unpublish` | ts, sessionId |

Privacy of the log itself: client IPs are truncated to /24 (IPv4) or /48
(IPv6), and user agents are stored as a short hash. The owner-only audit view
lists grants with labels, creation, last use and revoke buttons, newest first,
paginated. Tokens never appear in the log, only prefixes.

### 3.5 Abuse limits

| Surface | Limit |
| --- | --- |
| Redeem attempts | 10/min per IP, then 429 |
| Stream opens | 30/min per IP |
| Concurrent streams per share | 8 |
| Concurrent streams globally | 256 (503 + `Retry-After` when full) |
| Fixes served per stream | 50 000, then `end: slow`-style close |
| Share creation | 30/h per owner session |
| Ingest (Phase 1) | See §2.1 |

Failed redeems are throttled with the same sliding-window helper used for
login, keyed by the already-resolved client IP, so spoofed forwarding headers
only work behind a trusted proxy.

---

## 4. Milestones

Prerequisite for all of them: **PR #9 merged** (scratch runtime, binary
healthcheck, uid 10001, `/data` volume). Every milestone must keep
`cargo test`, `cargo clippy --all-targets -- -D warnings`, `npm run build` and
`docker build` green. Frontend has no test runner today; M1.5 adds a minimal
`node --experimental-strip-types` self-check rather than a new framework.

### Phase 1

**M1.1 — Live session model and owner endpoints**
Scope: `live` module, `LiveSession` in `AppState`, `live/{id}/session.json`,
`POST/GET/DELETE /api/live`, `/api/live/{id}`. Owner-session gated; unknown and
unauthorized ids both 404; ids random and validated.
Acceptance: `live_create_requires_session`, `live_ids_are_random`,
`live_sessions_survive_restart`; delete stops the session and keeps files.
Depends: PR #9.

**M1.2 — Fix log, ring buffer, retention primitives**
Scope: append-only NDJSON writer task, sparse `fixes.idx`, reader by sequence,
bounded ring, prune function. No HTTP.
Acceptance: `fix_log_round_trips_by_sequence`, `index_lands_on_256_boundaries`,
`ring_drops_oldest`, `prune_skips_promoted_sessions`; simulated 10 000 appends
produce an exact read-back.
Depends: M1.1.

**M1.3 — Device tokens and ingest endpoint**
Scope: `devices` module, token issue/list/revoke, `POST /api/ingest` JSON batch,
OsmAnd compat adapter, validation, dedupe, per-token and per-IP throttles,
256 KiB body limit.
Acceptance: `ingest_rejects_revoked_token`, `ingest_rejects_unknown_token`,
`ingest_batch_cap_413`, `ingest_body_limit_413`, `ingest_rate_limit_429`,
`ingest_drops_non_monotonic`, `ingest_dedupe_same_timestamp`,
`token_stored_as_hash_only`; a valid batch returns `201` with an accepted count.
Depends: M1.1, M1.2.

**M1.4 — SSE stream, backfill and resume**
Scope: `GET /api/live/{id}/stream`, `hello`/`snapshot`/`fix`/`end` events,
`Last-Event-ID` replay, reset snapshot, 15 s keepalive, bounded subscriber
channels, caps, no-store headers.
Acceptance: `stream_sends_hello_then_snapshot`, `stream_resumes_after_last_event_id`
(no duplicates, no gaps), `stream_resets_when_gap_exceeds_ring`,
`slow_subscriber_is_dropped`, `stream_404_for_unpublished_session`. Test client
uses `build_router` + `tokio::time`.
Depends: M1.2.

**M1.5 — Live viewer**
Scope: `/live/{id}` route, `src/live.ts`, incremental `viewer.appendTrack`,
live mode UI (badge, age, stale states), interpolated follow, reconnect UX,
`npm run test:frontend` self-check for fix → `TrackPoint` parity with
`enrichFlight` on a fixture.
Acceptance: build green; documented manual script (create session, `curl` 20
fixes, open the page, marker moves, HUD age updates, ingest stopped shows
"signal lost" within 60 s, ingest resumed resumes without a duplicated trail);
self-check passes.
Depends: M1.3, M1.4.

**M1.6 — Finish, promotion and pruning**
Scope: GPX generation, `POST /api/live/{id}/finish`, idle auto-finish, reuse of
`Store::write_flight`, `promotedSlug` idempotency, daily prune task wired in
`main`.
Acceptance: `finish_promotes_to_flight`, `finish_is_idempotent`,
`finish_requires_two_fixes`, `auto_finish_after_idle`, `prune_keeps_promoted_files`;
promoted GPX opens in the existing viewer and re-parses with current speeds and
vario.
Depends: M1.2.

**M1.7 — OGN/FLARM adapter (stretch, separate PR)**
Scope: read-only APRS-IS client behind a feature/env switch, mapping an
aircraft id to a device, reuse of the internal fix type and ingest writer.
Acceptance: adapter test with a recorded fixture feed; no network in tests;
disabled by default.
Depends: M1.3.

### Phase 2

**M2.1 — Share grants and redeem**
Scope: `shares` module, create/list/revoke, hashed storage, constant-time
lookup, expiry, `POST /api/shares/redeem` minting a scoped cookie.
Acceptance: `share_stores_hash_only`, `share_token_prefix_is_short`,
`share_redeem_expired_404`, `share_revoke_is_immediate`,
`share_lookup_is_constant_time` (length-equal compare path),
`share_token_never_logged`.
Depends: M1.1, M1.4.

**M2.2 — Public watch page**
Scope: `/watch` with fragment token, redeem flow, read-only viewer, noindex and
referrer headers, cookie scoped to `/api/live/`, fragment removed after redeem.
Acceptance: watch page connects with only the share cookie; no request carries
the token in a URL; revoked link fails on reload; page cannot trigger trim/tag
or finish.
Depends: M2.1.

**M2.3 — Privacy controls**
Scope: publish toggle, delay filter for snapshot and live events, origin radius
and landing trim, owner preview exempt.
Acceptance: `delayed_stream_never_emits_recent_fixes` (fake clock),
`origin_geofence_suppresses_prefix`, `finish_trims_landing_radius`,
`unpublished_session_has_no_stream`.
Depends: M2.2.

**M2.4 — Audit trail and owner view**
Scope: append-only audit writer, event set from §3.4, owner-only paginated view,
90-day prune, IP truncation and UA hashing.
Acceptance: `audit_records_share_lifecycle`, `audit_has_no_raw_ip`,
`audit_paginates_newest_first`, `audit_prune_respects_retention`.
Depends: M2.1.

**M2.5 — Abuse limits**
Scope: redeem/open throttles, concurrency caps, global connection ceiling with
503, counters exposed only through logs (no new public endpoint).
Acceptance: `redeem_throttle_429`, `share_concurrency_cap`, `global_stream_cap_503`,
`throttle_uses_resolved_client_ip` (spoofed headers ignored outside trusted
proxies).
Depends: M2.2.

**M2.6 — Per-person allowlists (optional)**
Scope: only if M2.1 grants are not enough — either named collaborators with
their own passwordless identity, or grants bound to a verified identity from an
external provider.
Acceptance: defined during scoping; must not regress the private-by-default
rule.
Depends: answer to open question 1.

---

## 5. Open questions for Pablo

1. Identity: keep the single shared password (share links only), or add
   per-person logins so "other app users" and per-viewer allowlists are real?
2. First ingest client: in-house mobile page/PWA, an existing OsmAnd/Traccar
   tracker app, or OGN/FLARM?
3. Should an idle unpromoted session auto-finish into a saved track, or be
   deleted after its retention window?
4. Publishing defaults: which delay values, and are the 2 km / 10 min origin
   and landing suppressions right for local sites?
5. May watch links work without any login (pure capability), or should a viewer
   also need the app password?
6. Storage: append-only files for both phases (recommended), or bundled SQLite
   from the start?
7. OGN/FLARM: is consuming public APRS data acceptable, and how should an
   aircraft id be tied to a person's session?
