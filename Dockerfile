# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.99-bookworm

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
        cmake \
        libclang-dev && \
    rm -rf /var/lib/apt/lists/*

# Install cargo-chef and cargo-auditable from their checksummed release binaries.
COPY ./hack/install-cargo-tool.sh /usr/local/bin/install-cargo-tool
RUN sh /usr/local/bin/install-cargo-tool cargo-chef /usr/local/bin && \
    sh /usr/local/bin/install-cargo-tool cargo-auditable /usr/local/bin

# Analyze project dependencies
FROM chef AS planner

COPY --parents \
    Cargo.toml \
    Cargo.lock \
    crates/zeta/Cargo.toml \
    crates/zeta/src/lib.rs \
    crates/zeta-plugin/Cargo.toml \
    crates/zeta-plugin/src/lib.rs \
    crates/zeta-cache/Cargo.toml \
    crates/zeta-cache/src/lib.rs \
    crates/zeta-http/Cargo.toml \
    crates/zeta-http/src/lib.rs \
    crates/zeta-mirror/Cargo.toml \
    crates/zeta-mirror/src/lib.rs \
    crates/zeta-url/Cargo.toml \
    crates/zeta-url/src/lib.rs \
    crates/zeta-utils/Cargo.toml \
    crates/zeta-utils/src/lib.rs \
    crates/plugins/Cargo.toml \
    crates/plugins/src/lib.rs \
    crates/coinmarketcap/Cargo.toml \
    crates/coinmarketcap/src/lib.rs \
    crates/dendanskeordbog/Cargo.toml \
    crates/dendanskeordbog/src/lib.rs \
    crates/reddit/Cargo.toml \
    crates/reddit/src/lib.rs \
    crates/zeta-test-support/Cargo.toml \
    crates/zeta-test-support/src/lib.rs \
    crates/zeta-unicode/Cargo.toml \
    crates/zeta-unicode/src/lib.rs \
    crates/zeta-unicode/build.rs \
    crates/zeta-unicode/build/build_names.rs \
    crates/kagi/Cargo.toml \
    crates/kagi/src/lib.rs \
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
    crates/zeta/Cargo.toml \
    crates/zeta/src \
    crates/zeta-plugin/Cargo.toml \
    crates/zeta-plugin/src \
    crates/zeta-cache/Cargo.toml \
    crates/zeta-cache/src \
    crates/zeta-http/Cargo.toml \
    crates/zeta-http/src \
    crates/zeta-mirror/Cargo.toml \
    crates/zeta-mirror/src \
    crates/zeta-url/Cargo.toml \
    crates/zeta-url/src \
    crates/zeta-utils/Cargo.toml \
    crates/zeta-utils/src \
    crates/plugins/Cargo.toml \
    crates/plugins/src \
    crates/plugins/migrations \
    crates/coinmarketcap/Cargo.toml \
    crates/coinmarketcap/src \
    crates/dendanskeordbog/Cargo.toml \
    crates/dendanskeordbog/src \
    crates/reddit/Cargo.toml \
    crates/reddit/src \
    crates/kagi/Cargo.toml \
    crates/kagi/src \
    crates/zeta-test-support/Cargo.toml \
    crates/zeta-test-support/src \
    crates/zeta-unicode/Cargo.toml \
    crates/zeta-unicode/src \
    crates/zeta-unicode/build \
    crates/zeta-unicode/build.rs \
    crates/zeta-unicode/data \
    ./
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo auditable build --release --locked --bin zeta && \
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
