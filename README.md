# W2UOS — Web3 Unified Operating System for Quantitative Trading

W2UOS is a Rust-native, modular, high-performance trading operating system designed to support the full lifecycle of quantitative trading systems, from simulation and backtesting to live exchange execution, with privacy, extensibility, and distributed deployment as first-class objectives.

## Overview

W2UOS provides a unified operating layer for quantitative trading infrastructure. It is built on an actor-based, asynchronous architecture with a structured message bus, enabling highly decoupled services for market data, order execution, strategy hosting, risk management, logging, and user interfaces.

The system is intended for both research and production use, and supports privacy-oriented networking, containerized deployment, and future multi-node clustering.

## Key Features

- Native Rust core with asynchronous, actor-oriented service model
- Modular service design: kernel, bus, market data, execution engine, strategy host, logging, API, UI, networking
- Simulation, paper-trading, and live trading modes
- Built-in support for major centralized exchanges such as OKX and Binance
- Structured logging, metrics, and historical data capture
- Web-based dashboard (WASM) for real-time monitoring
- Global risk control and circuit breaker system
- Distributed / multi-node architecture support
- Privacy-oriented network abstraction (direct, proxy, Tor, traffic shaping)
- Cross-platform deployment: Linux, macOS, Windows, Docker

## Repository Structure

W2UOS/
├── w2uos-kernel/       Core runtime and service orchestration
├── w2uos-bus/          Message bus abstraction
├── w2uos-data/         Market data ingestion and normalization
├── w2uos-exec/         Order execution engine and exchange adapters
├── w2uos-log/          Structured logging and telemetry
├── w2uos-api/          REST and WebSocket API
├── w2uos-dashboard/   WASM-based monitoring dashboard
├── w2uos-net/          Privacy-aware network abstraction
├── w2uos-backtest/    Historical replay and backtesting engine
├── wru-strategy/      Strategy runtime and implementations
├── w2uos-node/        Main node binary and configuration loader
├── config/            Development and production configuration
├── Dockerfile
├── docker-compose.yml
└── README.md

## Quick Start

### Prerequisites

- Rust toolchain (stable)
- Cargo
- Docker and docker-compose (optional, for container deployment)

### Local Build and Run (Simulation Mode)

```bash
git clone https://github.com/pb2n/W2UOS.git
cd W2UOS

export NODE_ENV=dev
export W2UOS_API_KEY="your-secret-key"

cargo run --release -p w2uos-node

The system will start in simulated market and paper-trading mode by default.
The REST API will be available at:

http://localhost:8080

Health check:

curl http://localhost:8080/health

Authenticated status query:

curl -H "X-API-KEY: your-secret-key" http://localhost:8080/status

By default, live trading is disabled. No real orders will be sent unless explicitly enabled in the configuration.

Docker Deployment

docker build -t w2uos:latest .
docker compose up -d

The service will be exposed on port 8080. Configuration is loaded from the config/ directory via volume mounting.

Configuration

Runtime settings are defined in TOML configuration files under the config/ directory, with environment-specific overrides.

Configurable parameters include:
	•	Network profile (direct, HTTP proxy, SOCKS5, Tor)
	•	API service and access control
	•	Market data subscriptions
	•	Execution mode (simulation, live OKX, live Binance)
	•	Risk limits and circuit breaker thresholds
	•	Logging and historical storage backends
	•	Strategy selection and parameters
	•	Node role and cluster settings

Refer to config/config.toml and config/config.dev.toml for concrete examples.

Backtesting and Historical Replay

W2UOS supports full historical replay through the backtest engine. The same execution and risk pipelines used in live trading are reused for backtesting to ensure behavioral consistency between research and production modes.

Backtest results include trade logs, PnL curves, drawdown statistics, latency metrics, and risk events.

Contributing

Contributions are welcome in the form of issues, pull requests, and design discussions.

Before submitting large changes, it is recommended to open an issue to discuss design impact and compatibility. All contributed code should follow the existing modular structure and configuration conventions.

License and Copyright

W2UOS is licensed under the Apache License, Version 2.0.

Copyright © 2025-12-05
Author: pb_2n^ (孔穆清)

Commercial use is permitted under the terms of the license. Proper attribution to the original author and project must be retained in all derived works and deployments.

See the LICENSE and NOTICE files for full legal terms and attribution requirements.

Contact

Author: pb_2n^ (孔穆清)
GitHub: https://github.com/pb2n
Project Repository: https://github.com/pb2n/W2UOS
