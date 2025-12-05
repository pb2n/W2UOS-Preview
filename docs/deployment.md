# W2UOS Deployment Guide

## Images
Build a production-style image with the included multi-stage Dockerfile:

```bash
docker build -t w2uos:latest .
```

The image ships a non-root user, copies the default configs to `/etc/w2uos`, and uses `/usr/local/bin/w2uos-node` as the entrypoint. The container expects `W2UOS_CONFIG_PATH=/etc/w2uos/config.toml` and respects `NODE_ENV` overlays.

## Local development via Compose

```bash
docker compose up --build
```

The compose stack runs:
- `w2uos` on port `8080`
- `postgres` on port `5432`
- (optional) commented NATS service ready for future clustering

Mounted configs:
- `./config/config.toml` → `/etc/w2uos/config.toml`
- `./config/config.dev.toml` → `/etc/w2uos/config.dev.toml`

Common environment variables:
- `NODE_ENV` (`dev`|`stage`|`prod`) selects the overlay file (`config.<env>.toml`).
- `W2UOS_API_KEY` overrides the API key.
- `W2UOS_HISTORY_URL` points the historical store (`sqlite:///...` or `postgres://...`).
- `W2UOS_NET_MODE`, `W2UOS_PROXY_URL` pick network modes for outbound traffic.

The container healthcheck hits `GET /health` without authentication.

## Configuration precedence
1. Base file: `/etc/w2uos/config.toml` (or `W2UOS_CONFIG_PATH`)
2. Overlay: `/etc/w2uos/config.<NODE_ENV>.toml` if present
3. Environment variables (e.g., `W2UOS_API_KEY`, `W2UOS_LOG_LEVEL`, `W2UOS_NET_MODE`)

## Running outside Docker

```bash
# ensure NODE_ENV if you want overlays
export NODE_ENV=dev
export W2UOS_CONFIG_PATH=./config/config.toml
cargo run -p w2uos-node
```

## Dashboard access
Point your browser to `http://localhost:8080` (or the mapped port) and supply the API key via header or query as implemented in the dashboard. WebSocket streaming uses `ws://<host>:8080/ws/stream`.
