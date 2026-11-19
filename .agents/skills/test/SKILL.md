---
name: test
description: Test flight_vis. Use when running cargo test, clippy, or the frontend build.
---

# Test

From the repo root:

```bash
cargo test --manifest-path server-rs/Cargo.toml
cargo clippy --manifest-path server-rs/Cargo.toml --all-targets -- -D warnings
npm ci && npm run build
```

`npm test` is the cargo test command. CI (`.github/workflows/ci.yml`) always runs `check` (cargo, frontend build, CSP scripts, amd64 `docker build`) and `docker-arm64`. The `e2e` job (Playwright) runs on pushes to `main` and on pull requests that are not drafts or carry the `ready` label.

Video round-trip tests spawn ffmpeg. If `ffmpeg` is missing they print `skipping video integration` and return. Slug, video-id, and body-limit tests do not need ffmpeg.

Flight route tests put bodies over 2 MB through `PUT /api/flights/{slug}`. That must stay 200. Axum's default 2 MB cap is raised to 64 MB only on the flight router.

`static_files::tests::join_resolves_subpaths` builds its own temp directory. Do not point it at a built `dist/`.
