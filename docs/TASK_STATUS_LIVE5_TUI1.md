# LIVE5 / TUI Phase 1 – Developer Status Brief

This document calibrates the current LIVE5 and TUI-1 state for future Codex tasks. W2UOS is a modular Rust trading OS: `w2uos-node` wires configuration into kernel services; `w2uos-data` streams exchange feeds into the bus/history; `w2uos-exec` handles simulated/paper/live execution; `w2uos-api` exposes authenticated REST control/metrics; `w2uos-tui` renders a read-only terminal dashboard that polls the API. Strategy logic lives separately in `wru-strategy` and should remain untouched unless explicitly requested.

## Completed scope (LIVE5 + TUI-1)
- **Execution/trading state model**: Unified `ExecutionMode` (`Sim`, `Paper`, `Live`) and kernel-level `TradingState` with risk profiles/limits, live-switch gate, and audit-logged control actions (pause/resume/freeze/unfreeze/flatten, mode/risk switches).
- **Control-plane endpoints**: Authenticated via `X-API-KEY`; include pause/resume/freeze/unfreeze/flatten, live-switch toggle and inspection, risk-profile and trading-mode switching, and audit-log retrieval. `/health` remains unauthenticated.
- **Observability**: `/metrics/system` exposes node/runtime/risk metadata; `/metrics/market` and `/metrics/market/summary` surface snapshots, orderbook/trades, and per-symbol summaries; `/status` returns node/env/trading state. Health endpoint reports kernel and connectivity state.
- **Market data adapters**: OKX and Binance spot WebSocket clients exist with reconnect/backoff scaffolding and snapshot publishing into the bus/history; simulated sources remain for testing.
- **Execution layer**: Paper backend fills from live snapshots with configurable slippage; live execution paths are wired with REST signing and private WS handling but guarded by execution mode and live-switch/risk gates. Paper remains the default for `config.live-okx-paper.toml`.
- **TUI (Phase 1)**: `w2uos-tui` polls the API and renders system/market/position panels at a fixed cadence, showing kernel mode, exchange, execution mode, live-switch, WS/REST health, snapshot rate, and basic position rows. No trading controls are exposed.
- **Configuration**: Example profile `config.live-okx-paper.toml` loads exchange URLs and symbol lists; secrets are read from environment (`W2UOS_OKX_API_KEY`, `W2UOS_OKX_API_SECRET`, `W2UOS_OKX_PASSPHRASE`).

## Pending work / recommended next steps
- **Live market fidelity**: Verify `MarketDataMode::LiveOkx` and `MarketDataMode::LiveBinanceSpot` consume only real exchange frames—remove or fence off any synthetic generators still reached in live modes. Confirm ticker/book/trade subscriptions and snapshot mapping (instId/symbol parsing, volume, timestamps) match exchange payloads.
- **Snapshot visibility**: Ensure a cache of latest `MarketSnapshot` per (exchange, symbol) is queryable by API/TUI; align `/market/snapshot` (or equivalent) with the TUI’s expected path/shape so the MARKET panel shows live prices instead of “waiting” placeholders.
- **Private channel depth**: Harden OKX private WS (orders/account/positions) and REST order routing for live execution; keep paper mode from hitting REST and retain live-switch/risk-gate enforcement before any real orders leave the node.
- **Robustness**: Strengthen reconnect/backoff logging for public/private WS, including subscription replay and parse error handling suitable for 24/7 operation; add lightweight tests for symbol parsing and payload-to-snapshot conversion.
- **TUI enrichments**: Fill positions panel with entry/mark/PnL/margin details once backend provides them; keep refresh cadence and API contract stable so the current panels remain functional during iterations.
- **Documentation/ops**: Provide operator notes for expected env vars, health checks, and typical curl probes (`/health`, `/metrics/system`, `/metrics/market*`, control endpoints with `X-API-KEY`).

## Constraints and invariants for future tasks
- Preserve existing API contracts: `X-API-KEY` for control/metrics endpoints; `/health` stays open. Do not break TUI expectations or endpoint shapes without coordinated updates.
- Do not modify strategy logic (`wru-strategy`) or alter trading/risk math unless explicitly requested.
- Paper mode must never reach real exchange REST; live orders must continue to require execution mode `Live` plus live-switch and risk approvals.
- Keep configuration-driven behavior intact (exchange selection, execution mode, symbol lists, env-based secrets); avoid introducing hardcoded secrets or behaviors.
