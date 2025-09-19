ARG RUST_VERSION=1.90-bookworm
FROM rust:${RUST_VERSION} AS cache

WORKDIR /usr/src/zeta

ADD zeta/Cargo.toml zeta/Cargo.toml
COPY Cargo.toml Cargo.lock .
COPY stub stub

RUN mkdir -p zeta/src && echo '' > zeta/src/lib.rs && cargo fetch

FROM cache AS builder

RUN cargo install cargo-auditable

COPY . .

RUN cargo auditable build --release

FROM gcr.io/distroless/cc-debian12

COPY --from=builder /usr/src/zeta/target/release/zeta .
COPY config.toml .

ENTRYPOINT ["./zeta"]
