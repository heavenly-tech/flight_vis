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
| `GET /api/flights` (library listing) | Session |
| `POST /api/flights` (multi-file upload) | Session |
| `PUT /api/flights/:slug` (save current track) | Session |
| `PATCH /api/flights/:slug` (rename / change slug) | Session |
| `DELETE /api/flights/:slug` | Session |
| `GET/POST /login`, `GET/POST /logout`, `GET /api/auth/me` | Public |

Set `FLIGHT_VIS_PASSWORD` on the host. If it is unset, existing slugs still render and upload/save stay locked.

1. Open [`/login`](https://flight.heavenly.cl/login) and enter the password.
2. Session cookie `fv_session` is HttpOnly, SameSite=Lax, 7 days (Secure behind HTTPS).
3. After login, **Library** lists tracks stored under `FLIGHT_DATA`: open one in the viewer, rename (name or shareable slug), or delete (with confirmation). Changing a slug updates the URL; a taken slug needs an overwrite confirm so it is not clobbered by accident.
4. **Library → Upload tracks** (or drop several `.igc` / `.gpx` files) saves each one with a slug from the filename. If that slug exists, the server adds `-2`, `-3`, …
5. **File → Open track** still previews one local file without writing. Pick a slug and **Save & copy link** to persist the current viewer state (track, trim, tags).

Public slug pages (`/vuelo-5`) stay viewable without login. Log out from the top bar or `GET/POST /logout`.

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

Open `http://localhost:5173/vuelo-5`. After login the **Library** panel lists saved flights. Drag the profile handles to crop taxi, **Skip ground** to auto-trim, **Tag** to mark events, **Save** to keep the current slug.

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
3. Confirm [flight.heavenly.cl/login](https://flight.heavenly.cl/login) accepts the password, then **Library** (list / upload / rename / delete) or **File → Open track → Save & copy link**.
4. Leave nginx-proxy as host routing only. Do **not** add `htpasswd`. Do **not** enable GitHub Pages.

Existing shareable slugs stay publicly viewable. The flight list and all writes require a session.

## Attribution

Imagery and elevation: Esri, Maxar, Earthstar Geographics, USGS, NOAA. Globe engine: CesiumJS (Apache 2.0).
