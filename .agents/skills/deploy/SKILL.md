---
name: deploy
description: Build and deploy the flight_vis image. Use when changing the Dockerfile or docker-compose, or when shipping the app.
---

# Deploy

```bash
docker build .
docker compose up -d --build
```

`.dockerignore` excludes `server-rs/target` so a local debug build is not copied over the release target inside the image.

The image has three stages:

1. `node:22-alpine` runs `npm ci` and `npm run build`.
2. `rust:1.98.0-alpine` builds a release binary with cargo-chef. Do not pass `--target x86_64-unknown-linux-musl`. Alpine's rustc already targets musl for the host arch, so ARM and x86_64 both work. The binary is `target/release/flight_vis`.
3. `alpine:3` runtime. `ffmpeg` is installed. `wget` (busybox) is the healthcheck. There is no `NODE_ENV`.

Runtime process:

- User id `10001` (group `10001`, name `app`). Not root.
- Owns `/data` (`chown 10001:10001`).
- `PORT=8080`, `FLIGHT_DATA=/data`, `FLIGHT_VIS_DIST=/app/dist`.
- `HEALTHCHECK` runs `wget -q -O /dev/null http://127.0.0.1:8080/api/health`.

Compose publishes the container on the external `proxy_network` (`VIRTUAL_HOST` / `LETSENCRYPT_HOST` = `flight.heavenly.cl`) and mounts `./data` on `/data`. A host mount replaces the image's `/data` directory; the mount must be writable by uid 10001.

Do not deploy from a review task unless that task explicitly says to.
