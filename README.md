# W2UOS — Web3 Unified Operating System for Quantitative Trading

W2UOS is a Rust-native, modular, high-performance trading operating system designed to support the full lifecycle of quantitative trading systems, from simulation and backtesting to live exchange execution, with privacy, extensibility, and distributed deployment as first-class objectives.

🚧 This is a Preview / Architecture Demonstration Repository

Any production deployment, commercial service, or derivative trading system
based on this preview requires separate authorization.

This repository is a preview and architectural demonstration version of W2UOS.
It does not contain full live trading logic, private execution modules, or production credentials. 

---

## 1. Overview

W2UOS provides a unified operating layer for quantitative trading infrastructure. It is built on an actor-based, asynchronous architecture with a structured message bus, enabling highly decoupled services for market data ingestion, order execution, strategy hosting, global risk management, logging and telemetry, REST and WebSocket APIs, and web-based dashboards.

The system is intended for both research and production use. It supports privacy-oriented networking, containerized deployment, and future multi-node clustering.

---

## 2. Key Features

- Native Rust core with asynchronous, actor-oriented service model  
- Modular service design: kernel, bus, market data, execution engine, strategy host, logging, API, UI, networking  
- Simulation, paper-trading, and live trading modes  
- Built-in support for major centralized exchanges such as OKX and Binance  
- Structured logging, metrics, and historical data capture  
- Web-based dashboard (WASM) for real-time monitoring  
- Global risk control and circuit breaker system  
- Distributed and multi-node architecture support  
- Privacy-oriented network abstraction (direct, proxy, Tor, traffic shaping)  
- Cross-platform deployment: Linux, macOS, Windows, Docker  

---

## 3. Repository Structure

W2UOS/  
w2uos-kernel/       Core runtime and service orchestration  
w2uos-bus/          Message bus abstraction  
w2uos-data/         Market data ingestion and normalization  
w2uos-exec/         Order execution engine and exchange adapters  
w2uos-log/          Structured logging and telemetry
w2uos-api/          REST and WebSocket API
w2uos-dashboard/   WASM-based monitoring dashboard
w2uos-tui/         Terminal dashboard for local live validation
w2uos-net/          Privacy-aware network abstraction
w2uos-backtest/    Historical replay and backtesting engine
wru-strategy/      Strategy runtime and implementations
w2uos-node/        Main node binary and configuration loader  
config/            Development and production configuration  
Dockerfile  
docker-compose.yml  
README.md  

---

flowchart LR
  Client[Client / Tools]
  subgraph Node["W2UOS Node"]
    API["w2uos-api\n(REST / future WS)"]
    Kernel["w2uos-kernel\n(service registry & bus)"]
    Config["config service"]
    Market["market-data service"]
    Strategy["strategy service"]
    Risk["risk service"]
    Exec["execution service"]
    Log["log service"]
  end
  DB[SQLite\nlog_events / trades / latency]
  Exchange[OKX / other exchanges]

  Client <--> API
  API <--> Kernel
  Kernel --> Config
  Kernel --> Market
  Kernel --> Strategy
  Kernel --> Risk
  Kernel --> Exec
  Kernel --> Log
  Market --> Kernel
  Exec --> Exchange
  Log --> DB
  Exec --> DB

---

## 4. Quick Start

### 4.1 Prerequisites

Rust toolchain (stable)  
Cargo  
Docker and docker-compose (optional)

---

### 4.2 Local Build and Run (Simulation Mode)

git clone https://github.com/pb2n/W2UOS.git  
cd W2UOS  

export NODE_ENV=dev  
export W2UOS_API_KEY="your-secret-key"  

cargo run --release -p w2uos-node  

The system starts in simulated market and paper-trading mode by default.  
REST API: http://localhost:8080  

Health check:  
curl http://localhost:8080/health  

Authenticated status query:
curl -H "X-API-KEY: your-secret-key" http://localhost:8080/status

LIVE-3 live + paper trading visibility (read-only, requires `X-API-KEY`):
- Live status: `curl -H "X-API-KEY: your-secret-key" http://localhost:8080/live/status`
- Paper positions: `curl -H "X-API-KEY: your-secret-key" http://localhost:8080/live/positions`
- Recent paper orders: `curl -H "X-API-KEY: your-secret-key" "http://localhost:8080/live/orders?limit=100"`

LIVE-4 control plane (requires `X-API-KEY` trader/admin roles):
- Pause trading: `curl -X POST -H "X-API-KEY: your-secret-key" -H "Content-Type: application/json" -d '{"reason":"manual_pause"}' http://localhost:8080/control/pause`
- Resume trading: `curl -X POST -H "X-API-KEY: your-secret-key" -H "Content-Type: application/json" -d '{"reason":"resume_after_check"}' http://localhost:8080/control/resume`
- Freeze trading: `curl -X POST -H "X-API-KEY: your-secret-key" -H "Content-Type: application/json" -d '{"reason":"emergency_freeze","cancel_open_orders":true}' http://localhost:8080/control/freeze`
- Unfreeze (safe states only): `curl -X POST -H "X-API-KEY: your-secret-key" -H "Content-Type: application/json" -d '{"reason":"manual_unfreeze","target_state":"SIMULATING"}' http://localhost:8080/control/unfreeze`
- Flatten request: `curl -X POST -H "X-API-KEY: your-secret-key" -H "Content-Type: application/json" -d '{"reason":"flatten_all_positions","mode":"market","timeout_ms":10000}' http://localhost:8080/control/flatten`
- Audit log: `curl -H "X-API-KEY: your-secret-key" "http://localhost:8080/control/audit-log?limit=50"`

Live trading is disabled by default.

---

### 4.3 Docker Deployment

docker build -t w2uos:latest .
docker compose up -d

Service port: 8080

---

### 4.4 Terminal TUI (LIVE validation, read-only)

The `w2uos-tui` binary renders a three-panel terminal dashboard (system, market, positions) by polling the local API.

```
cargo run -p w2uos-tui
```

Environment overrides:

- `W2UOS_API_BASE` (default: `http://localhost:8080`)
- `W2UOS_API_KEY` (required if API auth is enabled)
- `W2UOS_TUI_SYMBOL` (default: `BTC/USDT`)
- `W2UOS_TUI_HZ` (refresh rate; default: 8 Hz)
- `W2UOS_TUI_LIMIT` (recent trade/sample limits; default: 50)

Press `q` or `Esc` to exit. The TUI does not submit orders; it is intended for local LIVE pipeline validation only.

---

## 5. Configuration

Runtime settings are defined in TOML configuration files under the config/ directory, with environment-specific overrides.

Configurable parameters include:

Network profile (direct, HTTP proxy, SOCKS5, Tor)  
API service and access control  
Market data subscriptions  
Execution mode (simulation, live OKX, live Binance)  
Risk limits and circuit breaker thresholds  
Logging and historical storage backends  
Strategy selection and parameters  
Node role and cluster settings  

Refer to config/config.toml and config/config.dev.toml for examples.

---

## 6. Backtesting and Historical Replay

W2UOS supports full historical replay through the backtest engine.  
The same execution and risk pipelines used in live trading are reused for backtesting to ensure behavioral consistency between research and production modes.

Backtest results include:

Trade logs  
PnL curves  
Drawdown statistics  
Latency metrics  
Risk and circuit breaker events  

---

## 7. Contributing

Contributions are welcome through issues, pull requests, and architectural discussions.

Large changes should be discussed in advance.  
All contributed code must follow the modular architecture and configuration conventions.

---

## 8. License and Copyright

W2UOS is licensed under the Apache License, Version 2.0.

Copyright © 2025-12-05  
Author: pb_2n^ (孔穆清)

Commercial use is permitted under the terms of the license.  
Proper attribution to the original author and project must be retained in all derived works.

Logo and Branding Notice

The W2UOS name and logo are part of the W2UOS project identity.
Any use of the W2UOS logo outside of this repository must include
clear attribution to the original author.

Commercial use of the W2UOS logo is not permitted without
explicit written authorization from the author.

See the LICENSE and NOTICE files for full legal terms.

---

## 9. Contact

Author: pb_2n^ (孔穆清)  
GitHub: https://github.com/pb2n  
Project Repository: https://github.com/pb2n/W2UOS  

The historical Python strategy files previously included in this repository are published for academic and reference purposes only.
All commercial use, derivative trading systems, or production deployment based on these strategies require explicit written authorization from the author.


