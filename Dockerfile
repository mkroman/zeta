# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.98-bookworm

# Base layer with build tools
FROM rust:${RUST_VERSION} AS chef
WORKDIR /usr/src/app
ENV CARGO_TERM_COLOR=always \
    CARGO_INCREMENTAL=0 \
    CARGO_NET_RETRY=10 \
    RUSTUP_MAX_RETRIES=10

# BoringSSL (wreq, titles plugin) needs cmake, and bindgen needs libclang.
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        binutils \
        cmake \
        libclang-dev && \
    rm -rf /var/lib/apt/lists/*

# Install cargo-chef and cargo-auditable from their checksummed release binaries.
COPY hack/install-cargo-tool.sh /usr/local/bin/install-cargo-tool
COPY hack/strip-release.sh /usr/local/bin/strip-release
RUN sh /usr/local/bin/install-cargo-tool cargo-chef /usr/local/bin && \
    sh /usr/local/bin/install-cargo-tool cargo-auditable /usr/local/bin

# Analyze project dependencies
FROM chef AS planner

COPY --parents \
    Cargo.toml \
    Cargo.lock \
    zeta/Cargo.toml \
    zeta/src/lib.rs \
    zeta-plugin/Cargo.toml \
    zeta-plugin/src/lib.rs \
    dendanskeordbog/Cargo.toml \
    dendanskeordbog/src/lib.rs \
    reddit/Cargo.toml \
    reddit/src/lib.rs \
    kagi/Cargo.toml \
    kagi/src/lib.rs \
    ./

RUN cargo chef prepare --recipe-path recipe.json

# Build application binary
FROM chef AS builder
COPY --from=planner /usr/src/app/recipe.json recipe.json

# Cook dependencies — cached as long as recipe.json is unchanged.
# Cache mounts speed up cold builds and partial cache hits.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo chef cook --release --recipe-path recipe.json

# Now bring in real sources and build the app. Only copy build inputs so
# unrelated changes (config.toml, docs, ...) don't invalidate this layer.
COPY --parents \
    Cargo.toml \
    Cargo.lock \
    zeta/Cargo.toml \
    zeta/src \
    zeta/migrations \
    zeta-plugin/Cargo.toml \
    zeta-plugin/src \
    dendanskeordbog/Cargo.toml \
    dendanskeordbog/src \
    reddit/Cargo.toml \
    reddit/src \
    kagi/Cargo.toml \
    kagi/src \
    ./
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo auditable build --release --locked --bin zeta && \
    strip-release target/release/zeta && \
    cp target/release/zeta /usr/local/bin/zeta

# Runtime image with `yt-dlp` and `ffmpeg` for the tiktok plugin's video mirroring.
FROM debian:trixie-slim

# `yt-dlp` is installed from PyPI rather than apt, since TikTok extraction breaks regularly and
# the distro-packaged version goes stale between Debian releases.
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        ffmpeg \
        python3 \
        python3-pip && \
    rm -rf /var/lib/apt/lists/* && \
    pip3 install --no-cache-dir --break-system-packages yt-dlp

LABEL org.opencontainers.image.title="zeta" \
      org.opencontainers.image.description="An opinionated IRC bot with a bunch of plugins" \
      org.opencontainers.image.licenses="MIT,Apache-2.0" \
      org.opencontainers.image.vendor="Mikkel Kroman <mk@maero.dk>"

WORKDIR /app

COPY --from=builder /usr/local/bin/zeta .
COPY config.toml .

# Run as an unprivileged user.
RUN useradd --system --user-group --home-dir /app zeta
USER zeta

ENTRYPOINT ["/app/zeta"]
