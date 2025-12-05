# syntax=docker/dockerfile:1

FROM rust:1.75-slim as builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev libsqlite3-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY w2uos-* wru-strategy ./
COPY config ./config
RUN cargo build --release -p w2uos-node && strip target/release/w2uos-node

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 libsqlite3-0 curl \
    && rm -rf /var/lib/apt/lists/*
RUN useradd -m -u 1000 w2uos
WORKDIR /app
COPY --from=builder /app/target/release/w2uos-node /usr/local/bin/w2uos-node
COPY config /etc/w2uos
ENV W2UOS_CONFIG_PATH=/etc/w2uos/config.toml
ENV NODE_ENV=prod
USER w2uos
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 CMD curl -fsS http://localhost:8080/health || exit 1
ENTRYPOINT ["/usr/local/bin/w2uos-node"]
