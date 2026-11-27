# flight_vis

3D viewer for IGC and GPX tracks. Satellite globe, terrain, altitude-colored path, replay, trim, event tags, and shareable links.

Host your own instance, add to your library, and share tracks as you wish. You can tag along the track, trim the track start and end, and choose coloring for the track based on height, speed, or vertical velocity.

Live: [flight.heavenly.cl](https://flight.heavenly.cl) · sample flight [`/vuelo-5`](https://flight.heavenly.cl/vuelo-5)

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

Copy [`.env.example`](.env.example) to `.env`. Do not commit `.env`. Postal SMTP credentials can be set with `SMTP_PASS` or read from `SMTP_PASS_FILE` (trailing newline trimmed).

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

## Privacy

Club mode stores pilot names and flight metadata in the club database. How names appear depends on the viewer and each flight’s **show pilot name publicly** flag (`show_pilot_name_public`, set per flight when visibility is public).

| Surface | Own club / own flight | Other pilots (same club) | Other clubs (admin map) |
| --- | --- | --- | --- |
| Anonymous / public map | Name only if opted in | Name only if opted in | N/A |
| Club map (approved pilot) | Full name | Name only if opted in | N/A |
| Admin map (club admin) | Full name for member flights | Full name for member flights | **Name only if that flight opted in**; otherwise tail/position without name |

On the **admin map**, cross-club tracks are treated like anonymous viewers for pilot names: a hosting club’s admin does not see another club’s pilot name unless that pilot opted in to public name display for that flight. Own-club semantics are unchanged. Server logic: `server-rs/src/club/visibility.rs` (`show_pilot_name_admin_map`).

## License

[MIT](LICENSE)

## Attribution

Imagery and elevation: Esri, Maxar, Earthstar Geographics, USGS, NOAA. Globe engine: CesiumJS (Apache 2.0).

The 3D globe (web viewer and `/embed/3d`) uses Esri’s keyless World Imagery and World Elevation services. [Esri’s terms](https://location.arcgis.com/help/licensing-and-attribution/) limit that keyless use to non-commercial projects or Esri software/subscriptions; flight_vis is not commercial at the moment.

If it goes commercial, revisit: switch to an [ArcGIS Location Platform](https://location.arcgis.com/pricing/) API key (2M free tiles/month, then paid), add the basemap API host to the Globe CSP, and handle the key’s referrer restriction vs the embed’s `Referrer-Policy: no-referrer`.
