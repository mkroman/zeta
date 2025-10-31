# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.91-bookworm

# Base layer with build tools
FROM rust:${RUST_VERSION} AS chef
WORKDIR /usr/src/app
RUN cargo install cargo-chef cargo-auditable --locked

# Analyze project dependencies
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Build and cache dependencies separately from source code
FROM chef AS dependencies
COPY --from=planner /usr/src/app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Build application binary
FROM chef AS builder
COPY --from=dependencies /usr/src/app/target target
COPY --from=dependencies /usr/local/cargo /usr/local/cargo
COPY . .
RUN cargo auditable build --release --locked

# Minimal runtime image with security hardening
FROM gcr.io/distroless/cc-debian12:nonroot

LABEL org.opencontainers.image.title="zeta" \
      org.opencontainers.image.description="An opinionated IRC bot with a bunch of plugins" \
      org.opencontainers.image.licenses="MIT,Apache-2.0" \
      org.opencontainers.image.vendor="Mikkel Kroman <mk@maero.dk>"

WORKDIR /app

COPY --from=builder /usr/src/app/target/release/zeta .
COPY --from=builder /usr/src/app/config.toml .

USER nonroot

ENTRYPOINT ["./zeta"]
