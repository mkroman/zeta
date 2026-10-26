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

# Every workspace member lives under `crates/`; the globs keep new crates from having to be
# listed here by hand. `cargo chef prepare` only reads manifests, so the stub `lib.rs` files are
# enough; `zeta-unicode`'s build script is the one non-manifest input the recipe needs.
COPY --parents \
    Cargo.toml \
    Cargo.lock \
    crates/*/Cargo.toml \
    crates/*/src/lib.rs \
    crates/zeta-unicode/build.rs \
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
# unrelated changes (config.toml, docs, ...) don't invalidate this layer. The globs cover every
# workspace member; the migrations and the UCD data are the only compile-time inputs outside a
# crate's `src`.
COPY --parents \
    Cargo.toml \
    Cargo.lock \
    crates/*/Cargo.toml \
    crates/*/src \
    crates/*/build.rs \
    crates/plugins/migrations \
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
