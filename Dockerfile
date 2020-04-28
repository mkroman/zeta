# Copyright (C) 2020, Mikkel Kroman <mk@maero.dk>
FROM rust:latest as builder

WORKDIR /usr/src

COPY . .

RUN cargo build --release
RUN strip --strip-all target/release/zeta

FROM debian:buster

RUN apt update && \
  apt install -y openssl ca-certificates

COPY --from=builder /usr/src/target/release/zeta .
COPY config.yml .

ENTRYPOINT ./zeta
