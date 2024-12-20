FROM rust:latest AS cache

WORKDIR /usr/src/zeta

COPY zeta/Cargo.toml Cargo.lock .

RUN mkdir src && echo '' > src/lib.rs && cargo fetch

FROM cache AS builder

COPY . .

RUN cargo build --release

FROM gcr.io/distroless/cc-debian12

COPY --from=builder /usr/src/zeta/target/release/zeta .
COPY config.toml .

ENTRYPOINT ["./zeta"]
