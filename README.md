# flight_vis

3D flight visualization for **IGC** and **GPX** tracks. Built to match [SkyViz](https://skyviz.io) output: satellite globe, real terrain, altitude-colored path, and replay.

Vuelo5 (`public/samples/vuelo5.gpx`) loads automatically.

## Stack

- Vite 7 + TypeScript
- [CesiumJS](https://cesium.com/platform/cesiumjs/) globe
- Esri World Imagery + World Elevation (same family of data SkyViz uses)
- Optional Cesium ion terrain via `VITE_CESIUM_ION_TOKEN`

## Run locally

```bash
npm install
npm run dev
```

Open the printed URL (default `http://localhost:5173`). Drop another `.gpx` or `.igc` onto the page, or use **Open track**.

## Deploy

Static site. Any host that serves `dist/` works.

```bash
npm install
npm run build
npm run preview
```

- **Vercel / Netlify:** root of this repo, build command `npm run build`, output `dist`
- **GitHub Pages:** enable Pages from GitHub Actions; `.github/workflows/pages.yml` publishes `dist` on push to `main`

Terrain looks best with a free [Cesium ion](https://ion.cesium.com/) token:

```bash
echo 'VITE_CESIUM_ION_TOKEN=your_token' > .env.local
```

## Attribution

Imagery and elevation: Esri, Maxar, Earthstar Geographics, USGS, NOAA. Globe engine: CesiumJS (Apache 2.0).
