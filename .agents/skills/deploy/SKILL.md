---
name: deploy
description: Build and deploy the flight_vis image. Use when changing the Dockerfile or docker-compose, or when shipping the app.
---

# Deploy

```bash
docker build .
chown -R 10001:10001 ./data
docker compose up -d --build
```

`.dockerignore` excludes `server-rs/target` so a local debug build is not copied over the release target inside the image.

The image has four stages:

1. `node:22-alpine` runs `npm ci` and `npm run build`.
2. `rust:1.98.0-alpine` builds a release binary with cargo-chef. Do not pass `--target x86_64-unknown-linux-musl`. Alpine's rustc already targets musl for the host arch, so ARM and x86_64 both work. The binary is `target/release/flight_vis` and is statically linked.
3. `alpine:3` (`runtime-deps`) installs `ffmpeg`, `ca-certificates` and `musl-utils` (for `ldd`), then stages `/out`: the `ffmpeg`/`ffprobe` binaries, their full `ldd` shared-library closure (musl loader included), the CA bundle, `/etc/passwd` and `/etc/group`, `/data`, `/tmp`.
4. `scratch` runtime. Copies `/out`, the built `dist`, and `flight_vis`. There is no shell, package manager, `wget`, or `curl`.

Runtime process:

- User id `10001`, group `10001`, name `app`. The image config uses the numeric `10001:10001`; `/etc/passwd` only exists so tools that resolve the uid work.
- Owns `/data` (`COPY --chown` from the `runtime-deps` stage, plus a `.keep` file so the directory survives the copy).
- `PORT=8080`, `FLIGHT_DATA=/data`, `FLIGHT_VIS_DIST=/app/dist`, and the Alpine `PATH`.
- `HEALTHCHECK` runs `/app/flight_vis healthcheck`: the binary makes its own loopback `GET /api/health` (honouring `PORT`) and exits non-zero on a non-200 or non-`{"ok":true}` body. `scratch` has no `wget`, so this replaces the old busybox probe.
- `ffmpeg`/`ffprobe` live in `/usr/bin` and are the Alpine packages, so video transcode behaviour is unchanged. Their libraries are in `/lib` and `/usr/lib` as on Alpine.

There is no shell in the runtime image, so `docker exec -it <container> sh` fails. Use `docker exec <container> /app/flight_vis healthcheck` to probe, or `docker inspect --format '{{json .State.Health}}' <container>`.

Compose publishes the container on the external `proxy_network` (`VIRTUAL_HOST` / `LETSENCRYPT_HOST` = `flight.heavenly.cl`) and mounts `./data` on `/data`. A host mount replaces the image's `/data` directory; the mount must be writable by uid 10001. Before `docker compose up -d`, run `chown -R 10001:10001 ./data` on the host. The prod data folders are currently owned by root, so this step is required on the next deploy or the container cannot write the store.

Do not deploy from a review task unless that task explicitly says to.
