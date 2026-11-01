# Frontend: Vite + TypeScript bundle.
FROM node:22-alpine AS frontend
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
RUN npm run build

# Backend: Rust server. Alpine's rustc defaults to musl for the host
# architecture, so x86_64 and ARM both produce a static binary.
# cargo-chef keeps dependency compilation cached across rebuilds.
FROM rust:1.98.0-alpine AS chef
WORKDIR /app/server-rs
RUN apk add --no-cache musl-dev && cargo install cargo-chef --locked
COPY server-rs/ ./
RUN cargo chef prepare --recipe-path /recipe.json

FROM rust:1.98.0-alpine AS backend
WORKDIR /app/server-rs
RUN apk add --no-cache musl-dev
COPY --from=chef /usr/local/cargo/bin/cargo-chef /usr/local/cargo/bin/cargo-chef
COPY --from=chef /recipe.json /recipe.json
RUN cargo chef cook --release --recipe-path /recipe.json
COPY server-rs/ ./
RUN cargo build --release

# Runtime dependencies staged from Alpine. The final stage is `scratch`, so
# every shared object the app execs (ffmpeg/ffprobe for 480p transcodes) has
# to be copied along with its loader and library closure.
# COPY --chown/--chmod below creates the numeric user's dirs without `adduser`.
FROM alpine:3 AS runtime-deps
# musl-utils provides `ldd`, used to resolve ffmpeg's shared-library closure.
RUN apk add --no-cache ffmpeg ca-certificates musl-utils
RUN set -eux; \
    mkdir -p /out/bin /out/etc /out/lib /out/usr/lib /out/data /out/tmp; \
    cp /usr/bin/ffmpeg /usr/bin/ffprobe /out/bin/; \
    { ldd /usr/bin/ffmpeg; ldd /usr/bin/ffprobe; } \
      | awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^\//) print $i }' \
      | sort -u \
      | while read -r lib; do \
          mkdir -p "/out$(dirname "$lib")"; \
          cp -L "$lib" "/out$lib"; \
        done; \
    cp -a /lib/libc.musl-*.so.1 /out/lib/; \
    cp -a /lib/ld-musl-*.so.1 /out/lib/; \
    mkdir -p /out/etc/ssl; \
    cp -a /etc/ssl/certs /out/etc/ssl/; \
    cp -a /etc/ssl/cert.pem /out/etc/ssl/; \
    printf 'root:x:0:0:root:/root:/sbin/nologin\napp:x:10001:10001:app:/home/app:/sbin/nologin\n' > /out/etc/passwd; \
    printf 'root:x:0:\napp:x:10001:\n' > /out/etc/group; \
    chmod 1777 /out/tmp; \
    touch /out/data/.keep; \
    chown -R 10001:10001 /out/data

# Runtime: scratch. Static musl binary + frontend bundle + CA certs, ffmpeg and
# the exact shared-library closure it needs. No shell, package manager or
# wget/curl, so the healthcheck is a subcommand of the binary itself.
FROM scratch
WORKDIR /app
ENV PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
ENV PORT=8080
ENV FLIGHT_DATA=/data
ENV FLIGHT_VIS_DIST=/app/dist
COPY --from=runtime-deps /out/bin /usr/bin
COPY --from=runtime-deps /out/lib /lib
COPY --from=runtime-deps /out/usr /usr
COPY --from=runtime-deps /out/etc /etc
COPY --from=runtime-deps --chown=10001:10001 /out/data /data
COPY --from=runtime-deps --chmod=1777 /out/tmp /tmp
COPY --from=frontend /app/dist ./dist
COPY --from=backend /app/server-rs/target/release/flight_vis ./flight_vis
EXPOSE 8080
USER 10001:10001
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD ["/app/flight_vis", "healthcheck"]
CMD ["/app/flight_vis"]
