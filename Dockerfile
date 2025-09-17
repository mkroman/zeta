FROM rust:1.89-bookworm AS cache

WORKDIR /usr/src/zeta

ADD zeta/Cargo.toml zeta/Cargo.toml
COPY Cargo.toml Cargo.lock .
COPY stub stub

RUN mkdir -p zeta/src && echo '' > zeta/src/lib.rs && cargo fetch

FROM cache AS builder

COPY . .

RUN cargo build --release

FROM gcr.io/distroless/cc-debian12

COPY --from=builder /usr/src/zeta/target/release/zeta .
COPY config.toml .

ENTRYPOINT ["./zeta"]
