---
name: run
description: Run flight_vis locally. Use when starting the Rust API, the Vite UI, or the release binary.
---

# Run

From the repo root:

```bash
cp .env.example .env   # set FLIGHT_VIS_PASSWORD
npm install
npm run dev:api        # http://127.0.0.1:8080
npm run dev            # http://127.0.0.1:5173
```

`npm run dev:api` is `cargo run --manifest-path server-rs/Cargo.toml`. `npm start` is the release binary.

The API listens on `PORT` (default 8080) and serves `FLIGHT_VIS_DIST` (default `./dist`) plus the JSON API. Vite proxies `/api`, `/login`, and `/logout` to that API. A GET `/login` in dev is served by Vite as `login.html`; the Rust process serves `dist/login.html` or a small built-in page when that file is missing.

Open `http://localhost:5173/vuelo-5`. On first boot the server seeds slug `vuelo-5` from `public/samples/vuelo5.gpx` when the slug is absent.

Needs a Rust toolchain and, for video upload/transcode, `ffmpeg` and `ffprobe` on `PATH`. Without ffmpeg, track routes still run; video integration tests skip.
