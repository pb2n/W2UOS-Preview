# LIVE5 / TUI Phase 1 – Task Status

## Architecture overview
W2UOS is a modular Rust trading OS with distinct crates for the kernel/bus, market data adapters, execution backends, API, logging, and UI surfaces (web and terminal). The node binary wires these services together, exposing REST/WS interfaces while coordinating strategy, risk, and execution flows over an internal message bus.【F:README.md†L1-L87】

## Completed scope (LIVE5 + TUI-1)
- **Execution and trading state model**: ExecutionMode is unified to `Sim | Paper | Live`, while the kernel tracks lifecycle via `TradingState` plus risk profile/limit fields and live-switch flags for control-plane gating.【F:w2uos-exec/src/types.rs†L5-L40】【F:w2uos-kernel/src/lib.rs†L51-L122】
- **Control-plane APIs (authenticated via `X-API-KEY`)**: Pause/resume/freeze/unfreeze/flatten, risk profile and mode switches, live-switch toggling, audit-log retrieval, and control-state inspection are available under `/control/...` with health left unauthenticated.【F:w2uos-api/src/server.rs†L320-L401】【F:w2uos-api/src/server.rs†L368-L548】【F:w2uos-api/src/server.rs†L483-L507】
- **Observability endpoints**: `/health` returns connectivity state, while `/metrics/system` provides node/runtime/risk/aux subsystems data for dashboards.【F:w2uos-api/src/server.rs†L69-L75】【F:w2uos-api/src/server.rs†L540-L563】【F:w2uos-api/src/types.rs†L68-L132】
- **Market telemetry APIs**: Detailed and summary market metrics are exposed via `/metrics/market` and `/metrics/market/summary`, serving normalized snapshots, orderbook/trade samples, and per-symbol summaries for TUI/dashboards.【F:w2uos-api/src/server.rs†L557-L561】【F:w2uos-api/src/types.rs†L40-L65】
- **Terminal TUI (read-only)**: The `w2uos-tui` binary renders system/market/positions panels, pulling status/health/metrics endpoints at a configurable refresh rate; it shows live-switch state, WS/REST health, snapshot rate, and basic position rows without submitting trades.【F:w2uos-tui/src/main.rs†L538-L704】
- **Docs and quick start**: README documents node startup, health/status checks, LIVE-3/4 visibility and control endpoints, and TUI launch instructions with environment overrides.【F:README.md†L90-L162】

## Pending or recommended next steps
- **Market data fidelity**: Validate live exchange adapters end-to-end (e.g., OKX/Binance) and retire any legacy mock feeds in environments where real connectivity is expected.
- **Execution routing clarity**: Keep the paper/live split intact—ensure paper paths never hit real REST endpoints and that live paths honor the live-switch gate and risk approvals before sending orders.
- **TUI depth**: Fill in the positions panel with entry/mark/PnL/margin data once backend surfaces richer fields (currently rendered as `n/a` placeholders).【F:w2uos-tui/src/main.rs†L684-L703】
- **Control/metrics stability**: Preserve `/health` unauthenticated behavior and `X-API-KEY` enforcement elsewhere when adding new endpoints so existing dashboards and the TUI remain compatible.【F:w2uos-api/src/server.rs†L69-L75】【F:w2uos-api/src/server.rs†L483-L507】

## Constraints and invariants for future work
- Keep strategy crate (`wru-strategy`) untouched unless explicitly requested.
- Avoid breaking the authenticated API contract (header `X-API-KEY`) or the TUI’s expected endpoints/payload shapes when extending observability.
- Do not alter `/health` semantics or the live-switch/risk-gate controls without coordinating downstream consumers.
