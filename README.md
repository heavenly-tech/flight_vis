# flight_vis

3D viewer for IGC and GPX tracks. Satellite globe, terrain, altitude-colored path, replay, trim, event tags, and shareable links.

Host your own instance, add to your library, and share tracks as you wish. You can tag along the track, trim the track start and end, and choose coloring for the track based on height, speed, or vertical velocity.

Live: [flight.heavenly.cl](https://flight.heavenly.cl) · sample flight [`/demo`](https://flight.heavenly.cl/demo) (legacy [`/vuelo-5`](https://flight.heavenly.cl/vuelo-5) → 308)

Shareable slug pages are public. Saving, uploading, and the library need a login.

## Stack

- Vite 7 + TypeScript
- CesiumJS globe (Esri imagery and elevation)
- Axum (Rust) API that stores tracks on disk
- Password login with a signed session cookie

## Auth

| Route | Auth |
| --- | --- |
| `GET /:slug` and `GET /api/flights/:slug` | Public |
| Library list, upload, save, rename, delete | Session |

Set `FLIGHT_VIS_PASSWORD`. If it is unset, existing slugs still render and writes stay locked.

Copy [`.env.example`](.env.example) to `.env`. Do not commit `.env`.

Club registry (Postgres): self-serve signup at `/club-signup.html`, pilot registration per club at `/c/{slug}/registro`, and optional platform operators via `FLIGHT_SUPERADMIN_EMAILS` (OR `pilots.global_role = superadmin`). See `.env.example`.

## Local

```bash
cp .env.example .env
npm install
npm run dev:api
npm run dev
npm test
```

Needs a Rust toolchain for the API (`dev:api`, `test`, `start`). `npm test` runs the Rust suite; the API idles around 5 MB RSS.

Open `http://localhost:5173/vuelo-5`. Drag the profile handles to crop the track, **Skip ground** to auto-trim, **Tag** to mark events, **Save** to keep the current slug.

## Live location (Traccar Client)

Phase 1 server relay: stream a flight from your phone and watch it in the globe UI (PR B adds the client). Configure device tokens on the host, then point **Traccar Client** for Android (Flutter app, [traccar-client-sdk](https://github.com/traccar/traccar-client-sdk)) at this instance.

1. Generate a long random string (32+ hex chars) and map it to a display name:

   ```bash
   # in .env or a file pointed to by FLIGHT_VIS_DEVICE_TOKENS_FILE
   FLIGHT_VIS_DEVICE_TOKENS=a1b2c3d4e5f6...=My Pixel
   ```

2. In Traccar Client → **Settings**:
   - **Device identifier**: paste the same token (uploaded as form field `id`).
   - **Server URL**: `https://your-host/api/osmand` (the app POSTs `application/x-www-form-urlencoded` to this path).
   - **Frequency**: **1 second** (Settings → Accuracy / reporting interval).
   - **Battery**: allow background location, disable battery optimization for Traccar Client, and enable the foreground service option so fixes keep flowing in flight.

The server opens a live session on the first fix, relays over `GET /api/live/{id}/stream` (SSE) after operator login, keeps buffers 24 hours after stop, and auto-ends a session after 10 minutes without fixes. `POST /api/ingest` with `Authorization: Bearer <token>` accepts JSON batches from other clients.

## Deploy

```bash
docker compose up -d --build
```

Tracks are stored in `./data` (`FLIGHT_DATA` in Docker). The compose file expects an external Docker network named `proxy_network`.

The runtime image is `FROM scratch` and contains only the static Rust binary, the built frontend, CA certs, and `ffmpeg`/`ffprobe` with the libraries they need. There is no shell; the container healthcheck is `/app/flight_vis healthcheck`.

## License

[MIT](LICENSE)

## Attribution

Imagery and elevation: Esri, Maxar, Earthstar Geographics, USGS, NOAA. Globe engine: CesiumJS (Apache 2.0).
