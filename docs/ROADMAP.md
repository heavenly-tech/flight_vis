# Roadmap: live flight paths

Goal: a pilot can stream their position from a phone or logger, watch their own
flight on the globe as it happens, record it on their device, and share a live
view with chosen people or with a public live feed — without a server-side
library of user tracks and without adding a sidecar service.

Current state: **there is no live ingest, no stream endpoint, and no
client-side library model in the product yet.** The server today still stores
finished tracks on disk under `{FLIGHT_DATA}`; this roadmap moves **saved paths
and videos to the browser** and limits the server to **auth (where needed),
live relay, and short-lived buffers.**

---

## 1. Context

### Server

- One Axum 0.8 crate (`server-rs`), binary `flight_vis`, Tokio runtime. No
  database, no queue, no external process except `ffmpeg` for video transcodes
  (until video handling moves fully client-side).
- Routes today include flight CRUD on disk and video upload; **target state**
  drops the per-user flight library on the server. The server keeps health,
  optional login for operators, **live session relay**, ingest endpoints, and
  share/network publication metadata — not long-term track or video storage.
- `AppState` carries dist, `Auth`, bounded workers, throttles, and (new) in-memory
  live relay state. Background work stays inside the one binary (video worker
  during transition, live retention prune for transient buffers).

### Client-side storage (decision)

Each user's **saved paths and their videos** live in the browser only:

| Layer | Use | Limit / notes |
| --- | --- | --- |
| `localStorage` | Small metadata: library index, settings, share prefs | ~5 MB practical cap; no large blobs |
| **IndexedDB** or **OPFS** | Track files, recorded live paths, video blobs | Large payloads; pick one primary store per browser support |

The server **does not** maintain a library of user paths or host user videos in
the steady state. Export/import (file download/upload) is the mitigation when
browser data is cleared or the user changes device.

### Auth and watch links (decision)

- **Watch links open without login.** Viewers redeem a capability (link or public
  feed URL) and consume the live stream; they are not required to enter the app
  password.
- Operator actions (start/stop record, publish, device tokens) may still require
  a session when password mode is enabled; that is separate from watching.

### Frontend

- Vite + TypeScript + CesiumJS. The viewer loads tracks from **client storage**
  or from a **live SSE stream**, not from `GET /api/flights/{slug}` for the
  user's library.
- Live mode needs incremental track append, minimal-delay playback, and local
  persistence when a live session is **recorded**.

### Deployment and PR #9

- CI runs `cargo test`, `clippy`, `npm run build`, `docker build`.
- **PR #9 (open):** scratch runtime, static binary, uid `10001`, `/data` for
  **transient live buffers and config only**, not a multi-user track archive.
- No cron, sidecars, or shell in production; retention for relay buffers is a
  `tokio::time::interval` task in the binary.

### Design stance

- **Live data:** server holds a **short transient buffer** (ring + optional
  append log) only while a session is active or within a brief tail after stop;
  it **relays** fixes to subscribers. It does **not** become the system of
  record for recordings.
- **Recordings:** written on the **recording device** (or sharer's browser if
  they record while sharing) into IndexedDB/OPFS, with metadata in
  `localStorage`.
- **Streaming:** one-way after ingest; **SSE** remains the default transport
  (simple proxies, native reconnect, testable with `curl`).

---

## 2. Phase 1 — your own live path (record on device)

### 2.1 Ingest

HTTP push remains the primary model: `POST /api/ingest` (JSON batch) plus a
thin OsmAnd-style compat route for tracker apps that cannot set headers.
Device tokens authenticate ingest; limits and validation match a ~1 Hz fix
stream with batching for gaps.

**Open:** default tracker client — in-house PWA, OsmAnd, Traccar, or another
app — is **not decided** (see §6).

### 2.2 Server relay and transient buffer

While a session is **live**, the server:

- Accepts fixes, dedupes by `(device, t)`, and fans out to the owner's stream
  and (in Phase 2) to link/network subscribers.
- Keeps an in-memory ring (and optionally a small on-disk tail under `/data`)
  for reconnect backfill — **not** a permanent flight archive.

When the owner **records** the session, the **browser** appends fixes (and on
stop, finalizes a track + metadata) into client storage. The server may drop
buffered fixes after the session ends and retention for the transient tail
expires.

**After the stream ends (recorded live path):**

| Role | What happens |
| --- | --- |
| Server | Stops relay; transient buffer pruned per retention policy. No promoted slug on disk. |
| Recorder / sharer | Recording remains on device in IndexedDB/OPFS unless the user deletes it or clears site data. |
| Viewer | Sees live trail only while the stream is active unless a separate decision adds "viewer save copy" (open question). |

**Risk:** clearing browser storage **loses** local library and recordings.
**Mitigation:** export/import of tracks (and later videos) as files the user
controls.

### 2.2 Live viewing delay (decision)

**Minimal delay** for viewers and owner preview: fixes are emitted as soon as
validated (no intentional multi-minute hold). Optional privacy delay remains a
possible future knob but is **not** the default product stance.

### 2.3 Trim (decision)

Trim means **cut the last X minutes** of a track (live recording or imported
path), with **X configurable from 5 to 30** minutes. Applied in the client on
the stored track before display or export; no server-side trim store.

### 2.4 Frontend (live + record)

- Route `/live/{sessionId}` for the owner (session cookie or device-bound flow).
- `EventSource` consumer, incremental `appendTrack`, follow camera, stale
  detection.
- **Record** toggles local persistence; **Stop** finalizes into the client
  library.
- Trim UI uses the 5–30 minute "tail cut" rule.

---

## 3. Phase 2 — sharing live paths

Two publication modes (both can apply to the same live session):

### 3.1 Link share (decision: no login to watch)

- Owner creates a **capability link** (`/watch#t=…` or equivalent). Viewer
  redeems once (cookie scoped to stream API); **no app password**.
- Owner can **revoke** the link; open streams receive `end`; new redeems fail.
- While active, viewers see the **same minimal-delay** live trail as allowed by
  the grant. After stop/revoke/end, viewers see **nothing live**; they never
  had server-side copies of the recording unless viewer-side save is added later.

### 3.2 Network share ("public live feed")

**Definition:** a **network** share publishes the session to the **public live
feed** for this deployment — any visitor who can open the app (or a dedicated
`/live/public/…` or feed UI) **without a per-person link** can watch while the
session is marked **network-visible**.

| Aspect | Behaviour |
| --- | --- |
| **Who can see** | Anyone with access to the instance's public feed (unauthenticated watch allowed). Not the same as a private link; discovery is via feed/list UI, not secret URL entropy alone. |
| **Stop / unpublish** | Owner (authenticated operator) can **stop network visibility** or end the session. Relay stops for the feed; subscribers get `end`. |
| **After it stops** | Feed entry disappears or shows ended state; **no** server archive of the path. Recording, if any, stays only on devices that recorded locally. |
| **Link vs network** | Link = bearer secret, revocable per link. Network = broadcast to feed, broader audience, separate toggle and abuse limits. |

Shared technical pieces: SSE stream, redeem for link mode, feed subscription for
network mode, rate limits and concurrency caps on both.

### 3.3 Privacy and abuse (link + network)

- Unpublished sessions are owner-only (plus ingest).
- Origin/landing suppression and audit logging remain desirable; exact defaults
  for geofence radii are **open** (see §6).
- Throttles on ingest, redeem, stream opens, and global concurrent streams.

---

## 4. Milestones (high level)

Prerequisite: **PR #9 merged** where still relevant. Keep CI green.

**Phase 1**

1. Client library: IndexedDB/OPFS + `localStorage` index, import/export stub.
2. Live session + ingest + server relay buffer (no disk library writes).
3. SSE owner stream, minimal delay, record-to-client on stop.
4. Trim last 5–30 minutes in client.

**Phase 2**

5. Link share: create/revoke, watch without login, redeem cookie.
6. Network share: publish/unpublish, public feed UI, stop semantics as §3.2.
7. Abuse limits + optional audit for share/network events.

Detailed acceptance tests can be split per PR when implementing; this doc does
not fix server-side promotion to `{FLIGHT_DATA}/flights/{slug}`.

---

## 5. Migration note (today → target)

Until client storage ships, the existing disk `Store` and `/api/flights` API
remain in the codebase. New live work should **not** extend the server library
model; plan a later milestone to read legacy disk flights only for migration
export, then deprecate server-side user libraries.

---

## 6. Open questions

1. **Tracker client default:** in-house PWA, OsmAnd, Traccar, or other — which
   is the first-class ingest path?
2. **Viewer copy:** may a watch-only client **save a copy** of a live path to
   their own browser storage while watching, or only the recorder/sharer?
3. **Network feed scope:** single global list per deployment, tagged regions,
   or operator-curated entries?
4. **Idle live session:** auto-stop relay after inactivity, and should the
   client auto-finalize a partial recording or discard?
5. **Origin/landing suppression:** keep server-side geofence for link/network
   streams, and with what default radius/duration?
6. **Identity for operators:** keep single shared password for publish/record,
   or add per-person logins later?
7. **OGN/FLARM / APRS:** acceptable as a read-only ingest source, and how ids
   map to sessions?
8. **IndexedDB vs OPFS:** single strategy per browser tier, or OPFS-first with
   IndexedDB fallback?

---

## 7. Decisions log (owner)

| Topic | Decision |
| --- | --- |
| Saved paths & videos | Client-only: `localStorage` (~5 MB metadata), IndexedDB/OPFS for large data; no server library |
| Live + record | Live paths can be recorded; recording stored on recorder/sharer device |
| Share modes | Link (capability) or **network** (public live feed); see §3.2 |
| Watch links | No login required |
| Live delay | Minimal |
| Trim | Last X minutes, X ∈ [5, 30] |
| Tracker app | **Open** (not decided) |
