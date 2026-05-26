# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.95-bookworm

# Base layer with build tools
FROM rust:${RUST_VERSION} AS chef
WORKDIR /usr/src/app
ENV CARGO_TERM_COLOR=always \
    CARGO_INCREMENTAL=0 \
    CARGO_NET_RETRY=10 \
    RUSTUP_MAX_RETRIES=10

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo install cargo-chef cargo-auditable --locked

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

# Now bring in real sources and build the app.
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo auditable build --release --locked --bin zeta && \
    cp target/release/zeta /usr/local/bin/zeta

RUN cargo auditable build --release --locked

# Minimal runtime image with security hardening
FROM gcr.io/distroless/cc-debian12:nonroot

LABEL org.opencontainers.image.title="zeta" \
      org.opencontainers.image.description="An opinionated IRC bot with a bunch of plugins" \
      org.opencontainers.image.licenses="MIT,Apache-2.0" \
      org.opencontainers.image.vendor="Mikkel Kroman <mk@maero.dk>"

WORKDIR /app

COPY --from=builder /usr/local/bin/zeta .
COPY --from=builder /usr/src/app/config.toml .

USER nonroot

ENTRYPOINT ["/app/zeta"]
