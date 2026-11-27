# flight_vis CI base (fv-ci). Build on infra and tag e.g. fv-ci:rust-1.89-bookworm.
# Pre-pull on Woodpecker agents after publishing to avoid apt/rustup on every pipeline step.
FROM rust:1.89.0-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ffmpeg \
        libssl-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/* \
    && rustup component add rustfmt clippy