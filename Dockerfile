# Frontend: Vite + TypeScript bundle (unchanged).
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

# Runtime: static binary + frontend bundle + ffmpeg for video transcodes.
FROM alpine:3
WORKDIR /app
ENV PORT=8080
ENV FLIGHT_DATA=/data
ENV FLIGHT_VIS_DIST=/app/dist
RUN apk add --no-cache ffmpeg \
 && addgroup -g 10001 -S app \
 && adduser -S -D -H -u 10001 -G app app \
 && mkdir -p /data \
 && chown 10001:10001 /data
COPY --from=frontend /app/dist ./dist
COPY --from=backend /app/server-rs/target/release/flight_vis ./flight_vis
EXPOSE 8080
USER 10001
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD wget -q -O /dev/null http://127.0.0.1:8080/api/health || exit 1
CMD ["/app/flight_vis"]
