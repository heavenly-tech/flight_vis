# flight_vis agent guide

Rust (Axum) serves the API, session cookie, and built frontend. Vite + TypeScript is the globe UI. Tracks live on disk under `FLIGHT_DATA`.

Read the skill that matches the task before changing code:

- `.agents/skills/run/SKILL.md` — local API and UI
- `.agents/skills/test/SKILL.md` — Rust tests, clippy, frontend build
- `.agents/skills/deploy/SKILL.md` — Docker image and compose
- `.agents/skills/env/SKILL.md` — environment variables
- `.agents/skills/node-parity/SKILL.md` — response contract carried over from the Node server

Do not push to `main` or `cursor/rust-rewrite-video-sync` from a review branch. Do not force-push, merge, or deploy unless the task says to.

## Layout

- `server-rs/` — API binary `flight_vis`
- `src/` — viewer, library, track parsers
- `public/samples/vuelo5.gpx` — seeded sample when that slug is missing
- `dist/` — `npm run build` output, served as the SPA
- `data/` — default flight store (gitignored)

## Commands

```bash
npm install
npm run dev:api    # cargo run, port 8080
npm run dev        # Vite on 5173, proxies /api /login /logout
npm test           # cargo test
npm run build      # tsc --noEmit && vite build
npm start          # cargo run --release
cargo clippy --manifest-path server-rs/Cargo.toml --all-targets -- -D warnings
docker build .
docker compose up -d --build
```

Run Cargo and npm from the repo root so the default `data` and `dist` paths resolve.
