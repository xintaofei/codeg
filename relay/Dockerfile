FROM rust:1.85-bookworm AS builder

ARG CARGO_REGISTRIES_CRATES_IO_INDEX=sparse+https://index.crates.io/
ENV CARGO_REGISTRIES_CRATES_IO_INDEX=${CARGO_REGISTRIES_CRATES_IO_INDEX}
ENV CARGO_HTTP_TIMEOUT=600
ENV CARGO_NET_RETRY=5

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim

RUN useradd --system --uid 10001 --home-dir /nonexistent --shell /usr/sbin/nologin codeg-relay \
    && mkdir -p /data \
    && chown codeg-relay:codeg-relay /data
COPY --from=builder /build/target/release/codeg-relay /usr/local/bin/codeg-relay

USER codeg-relay
ENV CODEG_RELAY_BIND=0.0.0.0:8787
ENV CODEG_RELAY_CREDENTIAL_FILE=/data/device-credentials.json
EXPOSE 8787
VOLUME ["/data"]

ENTRYPOINT ["/usr/local/bin/codeg-relay"]
