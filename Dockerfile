# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.100-bookworm

# Official mise image, pinned by digest so the tool bootstrap is reproducible.
FROM ghcr.io/jdx/mise:2026.11.6@sha256:d4de1845114f6d73e90e15f7642885f74f3ca93074491038a2b9c43e4b897f21 AS mise

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
        ca-certificates \
        cmake \
        libclang-dev && \
    rm -rf /var/lib/apt/lists/*

# Install cargo-chef and cargo-auditable from their checksummed GitHub releases
# (prebuilt binaries — much faster in CI than compiling them). mise verifies
# both against mise.docker.lock before exposing their shims.
COPY --from=mise /usr/local/bin/mise /usr/local/bin/mise
ENV MISE_DATA_DIR=/mise \
    MISE_CONFIG_DIR=/mise \
    MISE_CACHE_DIR=/mise/cache \
    MISE_ENV=docker \
    PATH="/mise/shims:${PATH}"
COPY mise.docker.toml mise.docker.lock ./
RUN --mount=type=cache,target=/mise/cache \
    mise trust && \
    mise install --locked

# Analyze project dependencies
FROM chef AS planner

# Workspace members live under `crates/` plus the top-level `benchmarks/` and `xtask/`
# packages. The globs keep new crates and benchmarks from having to be listed by hand.
# `cargo chef prepare` only reads manifests, so the stub `lib.rs` files and the declared
# benchmark sources are enough; `zeta-unicode`'s build script is the one non-manifest input
# the recipe needs.
COPY --parents \
    Cargo.toml \
    Cargo.lock \
    crates/*/Cargo.toml \
    crates/*/src/lib.rs \
    crates/zeta-unicode/build.rs \
    benchmarks/Cargo.toml \
    benchmarks/benches/*.rs \
    xtask/Cargo.toml \
    xtask/src/lib.rs \
    ./

RUN cargo chef prepare --recipe-path recipe.json

# Build application binary
FROM chef AS builder
COPY --from=planner /usr/src/app/recipe.json recipe.json

# Cook dependencies — cached as long as recipe.json is unchanged.
# Cache mounts speed up cold builds and partial cache hits.
#
# cargo-chef 0.1.78 reconstructs the recipe's manifests with per-target `edition` keys and empty
# stubs, so Cargo 1.100 reports a deprecation warning per target (LukeMathWalker/cargo-chef#350,
# fixed by unmerged #369) and `cargo::unused_dependencies` warnings for dependencies the stubs do
# not use. Both are false positives for this step only. `CARGO_TERM_QUIET` hides the hard-coded
# manifest warning, and `CARGO_BUILD_WARNINGS` silences the Cargo lint; real builds are unaffected.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    CARGO_TERM_QUIET=true CARGO_BUILD_WARNINGS=allow \
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
    benchmarks/Cargo.toml \
    benchmarks/benches \
    xtask/Cargo.toml \
    xtask/src \
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
