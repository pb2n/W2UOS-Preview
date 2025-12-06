# W2UOS — Web3 Unified Operating System for Quantitative Trading

<img width="1024" height="1024" alt="Logo" src="https://github.com/user-attachments/assets/b2acd7b3-e739-4d21-be2d-11c98bb0310a" />

W2UOS is a Rust-native, modular, high-performance trading operating system designed to support the full lifecycle of quantitative trading systems, from simulation and backtesting to live exchange execution, with privacy, extensibility, and distributed deployment as first-class objectives.

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
w2uos-net/          Privacy-aware network abstraction  
w2uos-backtest/    Historical replay and backtesting engine  
wru-strategy/      Strategy runtime and implementations  
w2uos-node/        Main node binary and configuration loader  
config/            Development and production configuration  
Dockerfile  
docker-compose.yml  
README.md  

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

Live trading is disabled by default.

---

### 4.3 Docker Deployment

docker build -t w2uos:latest .  
docker compose up -d  

Service port: 8080  

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

See the LICENSE and NOTICE files for full legal terms.

---

## 9. Contact

Author: pb_2n^ (孔穆清)  
GitHub: https://github.com/pb2n  
Project Repository: https://github.com/pb2n/W2UOS  

The historical Python strategy files previously included in this repository are published for academic and reference purposes only.
All commercial use, derivative trading systems, or production deployment based on these strategies require explicit written authorization from the author.
