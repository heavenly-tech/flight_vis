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

Copy [`.env.example`](.env.example) to `.env`. Do not commit `.env`.

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

## Deploy

```bash
docker compose up -d --build
```

Tracks are stored in `./data` (`FLIGHT_DATA` in Docker). The compose file expects an external Docker network named `proxy_network`.

## License

[MIT](LICENSE)

## Attribution

Imagery and elevation: Esri, Maxar, Earthstar Geographics, USGS, NOAA. Globe engine: CesiumJS (Apache 2.0).
