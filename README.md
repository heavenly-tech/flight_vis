# flight_vis

3D IGC/GPX viewer at [flight.heavenly.cl](https://flight.heavenly.cl). Satellite globe, terrain, altitude-colored path, trim, event tags, and shareable slugs.

Production is **Docker only** (`VIRTUAL_HOST=flight.heavenly.cl`). Do not enable GitHub Pages.

Vuelo 5 lives at [`/vuelo-5`](https://flight.heavenly.cl/vuelo-5).

## Stack

- Vite 7 + TypeScript frontend
- CesiumJS globe (Esri imagery + elevation)
- Small Hono API that stores tracks on disk
- App login: password env + signed session cookie (same idea as [statements.heavenly.cl](https://statements.heavenly.cl))
- Docker on heavenly, routed by nginx-proxy (`VIRTUAL_HOST=flight.heavenly.cl`)

## Auth and upload

App-level login — **not** nginx `htpasswd`.

| Route | Auth |
| --- | --- |
| `GET /vuelo-5` and other shareable slugs | Public |
| `GET /api/flights/:slug` | Public |
| `GET /api/flights` (directory listing) | Session |
| `PUT /api/flights/:slug` (upload / save) | Session |
| `GET/POST /login`, `GET/POST /logout`, `GET /api/auth/me` | Public |

Set `FLIGHT_VIS_PASSWORD` on the host. If it is unset, existing slugs still render and upload/save stay locked.

1. Open [`/login`](https://flight.heavenly.cl/login) and enter the password.
2. Session cookie `fv_session` is HttpOnly, SameSite=Lax, 7 days (Secure behind HTTPS).
3. **File → Open track** accepts `.igc` / `.gpx` (the formats the viewer already parses). Local preview does not write to disk.
4. Pick a slug and **Save & copy link** to persist the track, trim, and tags. That `PUT` is the authenticated upload.

Log out from the File menu or `GET/POST /logout`.

### Environment

| Variable | Required | Purpose |
| --- | --- | --- |
| `FLIGHT_VIS_PASSWORD` | For upload/save | Shared app password. Never commit it. |
| `FLIGHT_DATA` | No (default `./data` or `/data` in Docker) | On-disk flight store |
| `PORT` | No (default `8080`) | API / static listen port |
| `VIRTUAL_HOST` / `LETSENCRYPT_HOST` / `VIRTUAL_PORT` | Docker on heavenly | nginx-proxy routing |

Copy [`.env.example`](.env.example) to `.env` next to `docker-compose.yml` (Compose reads it for interpolation). Keep `.env` off git.

## Local

```bash
cp .env.example .env   # set FLIGHT_VIS_PASSWORD
npm install
npm run dev:api        # API + seeded vuelo-5 on :8080
npm run dev            # Vite on :5173, proxies /api, /login, /logout
npm test
```

Open `http://localhost:5173/vuelo-5`. Drag the profile handles to crop taxi, **Skip ground** to auto-trim, **Tag** to mark events, **Save** (after login) to keep the slug.

Without a password, the globe and public slugs still work; Save stays locked.

## Deploy

```bash
docker compose up -d --build
```

On heavenly the stack is `/opt/stacks/app-heavenly-flight-vis` on `proxy_network`. Data is `./data`.

### Rita / Infra overlay after merge

Do this on the VPS stack, not in git:

1. Pull / rebuild the image (`docker compose up -d --build` in `/opt/stacks/app-heavenly-flight-vis`).
2. Put a strong password in the stack `.env` (never commit it):

   ```bash
   FLIGHT_VIS_PASSWORD=...
   ```

   Compose already interpolates `FLIGHT_VIS_PASSWORD` into the container. Recreate the container after changing it (sessions are signed with the password, so a change logs everyone out).
3. Confirm [flight.heavenly.cl/login](https://flight.heavenly.cl/login) accepts the password, then **File → Open track → Save & copy link**.
4. Leave nginx-proxy as host routing only. Do **not** add `htpasswd`. Do **not** enable GitHub Pages.

Existing shareable slugs stay publicly viewable. The flight list and all writes require a session.

## Attribution

Imagery and elevation: Esri, Maxar, Earthstar Geographics, USGS, NOAA. Globe engine: CesiumJS (Apache 2.0).
