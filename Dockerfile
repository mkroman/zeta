# Copyright (C) 2020, Mikkel Kroman <mk@maero.dk>
FROM debian:buster

RUN apt update && \
  apt install -y openssl ca-certificates

COPY /drone/src/target/release/zeta .

RUN strip --strip-all ./zeta

ENTRYPOINT ./zeta
