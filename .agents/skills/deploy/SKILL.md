---
name: deploy
description: Build and deploy the flight_vis image. Use when changing the Dockerfile or docker-compose, or when shipping the app.
---

# Deploy

```bash
docker build .
chown -R 65534:65534 ./data
docker compose up -d --build
```

`.dockerignore` excludes `server-rs/target` so a local debug build is not copied over the release target inside the image.

The image has four stages:

1. `node:22-alpine` runs `npm ci` and `npm run build`.
2. `rust:1.98.0-alpine` builds a release binary with cargo-chef. Do not pass `--target x86_64-unknown-linux-musl`. Alpine's rustc already targets musl for the host arch, so ARM and x86_64 both work. The binary is `target/release/flight_vis` and is statically linked.
3. `alpine:3` (`runtime-deps`) installs `ffmpeg`, `ca-certificates` and `musl-utils` (for `ldd`), then stages `/out`: the `ffmpeg`/`ffprobe` binaries, their full `ldd` shared-library closure (musl loader included), the CA bundle, `/etc/passwd` and `/etc/group`, `/data`, `/tmp`.
4. `scratch` runtime. Copies `/out`, the built `dist`, and `flight_vis`. There is no shell, package manager, `wget`, or `curl`.

Runtime process:

- User id `65534`, group `65534`, name `nobody`. The image uses numeric `65534:65534`; `/etc/passwd` only exists so tools that resolve the uid work.
- Owns `/data` (`COPY --chown` from the `runtime-deps` stage, plus a `.keep` file so the directory survives the copy).
- `PORT=8080`, `FLIGHT_DATA=/data`, `FLIGHT_VIS_DIST=/app/dist`, and the Alpine `PATH`.
- `HEALTHCHECK` runs `/app/flight_vis healthcheck`: the binary makes its own loopback `GET /api/health` (honouring `PORT`) and exits non-zero on a non-200 or non-`{"ok":true}` body. `scratch` has no `wget`, so this replaces the old busybox probe.
- `ffmpeg`/`ffprobe` live in `/usr/bin` and are the Alpine packages, so video transcode behaviour is unchanged. Their libraries are in `/lib` and `/usr/lib` as on Alpine.

There is no shell in the runtime image, so `docker exec -it <container> sh` fails. Use `docker exec <container> /app/flight_vis healthcheck` to probe, or `docker inspect --format '{{json .State.Health}}' <container>`.

Compose attaches the container to external `proxy_network` and `db_shared`, resolves `postal-smtp` via `extra_hosts: host-gateway`, and mounts `./data` on `/data`. The mount must be writable by uid `65534` (`chown -R 65534:65534 ./data` on the host). Postgres is the shared heavenly cluster (`DATABASE_URL`); this compose file does not start a local Postgres service.

## Android App Links (production)

The API does **not** assume a default Android package. To enable verified App Links, **Abrir en la app** on `/app.html`, and the optional intent button on the check-in landing page, set both variables on the host (`.env` or compose `environment`) before `docker compose up`:

```bash
FLIGHT_ANDROID_APP_PACKAGE=cl.heavenly.flightvis
FLIGHT_ANDROID_CERT_SHA256=<release signing cert SHA-256 fingerprint>
```

Obtain the fingerprint from the **flightvis-client** release keystore (same cert that signs Play/APK builds), for example:

```bash
keytool -list -v -keystore /path/to/release.keystore -alias <alias>
```

Copy the `SHA256:` line (colons optional; comma-separate if you rotate keys). Alternatively mount fingerprints via `FLIGHT_ANDROID_CERT_SHA256_FILE`.

When **both** package and fingerprint are set, the container serves `GET /.well-known/assetlinks.json` and `GET /api/app/install-link` includes `intentUrl`. Club check-in QR symbols always encode a plain HTTPS URL (`https://<host>/c/{slug}/q/{token}`) so camera apps can scan them; opening the native app relies on verified App Links plus the client manifest, not `intent://` in the QR.

When either variable is unset, `/.well-known/assetlinks.json` is **404**, **Abrir en la app** stays hidden, and check-in still works over the web flow.

Do not deploy from a review task unless that task explicitly says to.
