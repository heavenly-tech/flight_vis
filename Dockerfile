# Frontend: Vite + TypeScript bundle (unchanged).
FROM node:22-alpine AS frontend
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
RUN npm run build

# Backend: Rust server as a static musl binary. cargo-chef keeps dependency
# compilation cached across rebuilds.
FROM rust:1.98-alpine AS chef
WORKDIR /app/server-rs
RUN apk add --no-cache musl-dev && cargo install cargo-chef --locked
COPY server-rs/ ./
RUN cargo chef prepare --recipe-path /recipe.json

FROM rust:1.98-alpine AS backend
WORKDIR /app/server-rs
RUN apk add --no-cache musl-dev
COPY --from=chef /usr/local/cargo/bin/cargo-chef /usr/local/cargo/bin/cargo-chef
COPY --from=chef /recipe.json /recipe.json
RUN cargo chef cook --release --recipe-path /recipe.json --target x86_64-unknown-linux-musl
COPY server-rs/ ./
RUN cargo build --release --target x86_64-unknown-linux-musl

# Runtime: static binary + frontend bundle + ffmpeg for video transcodes.
FROM alpine:3
WORKDIR /app
ENV NODE_ENV=production
ENV PORT=8080
ENV FLIGHT_DATA=/data
RUN apk add --no-cache ffmpeg
COPY --from=frontend /app/dist ./dist
COPY --from=backend /app/server-rs/target/x86_64-unknown-linux-musl/release/flight_vis ./flight_vis
RUN mkdir -p /data
EXPOSE 8080
CMD ["/app/flight_vis"]
