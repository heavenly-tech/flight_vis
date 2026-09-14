# flight_vis

3D IGC/GPX viewer at [flight.heavenly.cl](https://flight.heavenly.cl). Satellite globe, terrain, altitude-colored path, trim, event tags, and shareable slugs.

Vuelo 5 lives at [`/vuelo-5`](https://flight.heavenly.cl/vuelo-5).

## Stack

- Vite 7 + TypeScript frontend
- CesiumJS globe (Esri imagery + elevation)
- Small Hono API that stores tracks on disk
- Docker on heavenly, routed by nginx-proxy (`VIRTUAL_HOST=flight.heavenly.cl`)

## Local

```bash
npm install
npm run dev:api   # API + seeded vuelo-5 on :8080
npm run dev       # Vite on :5173, proxies /api
```

Open `http://localhost:5173/vuelo-5`. Drag the profile handles to crop taxi, **Skip ground** to auto-trim, **Tag** to mark events, **Save** to keep the slug.

## Deploy

```bash
docker compose up -d --build
```

On heavenly the stack is `/opt/stacks/app-heavenly-flight-vis` on `proxy_network`. Data is `./data`.

## Attribution

Imagery and elevation: Esri, Maxar, Earthstar Geographics, USGS, NOAA. Globe engine: CesiumJS (Apache 2.0).
