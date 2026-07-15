FROM rust:1.85-bookworm AS builder

ARG CARGO_REGISTRY_MIRROR
ENV CARGO_HTTP_TIMEOUT=600
ENV CARGO_NET_RETRY=5

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN if [ -n "${CARGO_REGISTRY_MIRROR}" ]; then \
      mkdir -p .cargo; \
      printf '[source.crates-io]\nreplace-with = "deployment-mirror"\n[source.deployment-mirror]\nregistry = "%s"\n' \
        "${CARGO_REGISTRY_MIRROR}" > .cargo/config.toml; \
    fi \
    && cargo build --release --locked

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
