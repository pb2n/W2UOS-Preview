use std::sync::Arc;

use anyhow::Result;
use w2uos_backtest::{BacktestConfig, BacktestCoordinator};
use w2uos_bus::MessageBus;
use w2uos_data::TradingMode;
use w2uos_data::{ExchangeId, MarketSnapshot, Symbol};
use w2uos_exec::ExecutionService;
use w2uos_kernel::{ClusterService, Kernel, KernelState, RiskSupervisorService};
use w2uos_log::{types::LogEvent, LogService};
use w2uos_net::NetProfile;

use crate::types::{
    ClusterNodeDto, LatencyBucketDto, LatencySummaryDto, LiveOrderDto, LiveStatusDto, LogDto,
    NodeStatusDto, PositionDto, ServiceStatusDto,
};

#[derive(Clone)]
pub struct LivePipelineState {
    last_snapshot: Arc<tokio::sync::Mutex<Option<MarketSnapshot>>>,
    trades: Arc<tokio::sync::Mutex<Vec<LiveOrderDto>>>,
    symbols: Vec<String>,
}

impl LivePipelineState {
    pub fn new(symbols: Vec<String>) -> Self {
        Self {
            last_snapshot: Arc::new(tokio::sync::Mutex::new(None)),
            trades: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            symbols,
        }
    }

    pub fn start_listeners(&self, bus: Arc<dyn MessageBus>, subjects: Vec<String>) {
        let last_snapshot = Arc::clone(&self.last_snapshot);
        let symbol_filter = self.symbols.clone();
        tokio::spawn(async move {
            for subject in subjects {
                let mut sub = match bus.subscribe(&subject).await {
                    Ok(sub) => sub,
                    Err(_) => continue,
                };

                let last_snapshot = Arc::clone(&last_snapshot);
                let symbol_filter = symbol_filter.clone();
                tokio::spawn(async move {
                    while let Some(msg) = sub.receiver.recv().await {
                        if let Ok(snapshot) = serde_json::from_slice::<MarketSnapshot>(&msg.0) {
                            if symbol_filter.is_empty()
                                || symbol_filter.iter().any(|sym| {
                                    sym.eq_ignore_ascii_case(&format!(
                                        "{}-{}",
                                        snapshot.symbol.base, snapshot.symbol.quote
                                    ))
                                })
                            {
                                let mut guard = last_snapshot.lock().await;
                                *guard = Some(snapshot);
                            }
                        }
                    }
                });
            }
        });

        let trades_state = Arc::clone(&self.trades);
        tokio::spawn(async move {
            if let Ok(mut sub) = bus.subscribe("log.trade").await {
                while let Some(msg) = sub.receiver.recv().await {
                    if let Ok(trade) = serde_json::from_slice::<w2uos_log::TradeRecord>(&msg.0) {
                        let mut guard = trades_state.lock().await;
                        guard.push(trade.into());
                        if guard.len() > 200 {
                            let excess = guard.len() - 200;
                            guard.drain(0..excess);
                        }
                    }
                }
            }
        });
    }

    pub async fn last_snapshot(&self) -> Option<MarketSnapshot> {
        let guard = self.last_snapshot.lock().await;
        guard.clone()
    }

    pub async fn recent_trades(&self, limit: usize) -> Vec<LiveOrderDto> {
        let guard = self.trades.lock().await;
        let len = guard.len();
        let start = len.saturating_sub(limit);
        guard[start..].to_vec()
    }

    pub fn subscribed_symbols(&self) -> &[String] {
        &self.symbols
    }
}

#[derive(Clone)]
pub struct ApiContext {
    pub kernel: Arc<Kernel>,
    pub bus: Arc<dyn MessageBus>,
    pub log_service: Option<Arc<LogService>>,
    pub exec_service: Option<Arc<ExecutionService>>,
    pub cluster_service: Option<Arc<ClusterService>>,
    pub backtest: Option<Arc<BacktestCoordinator>>,
    pub market_subjects: Vec<String>,
    pub net_profile: NetProfile,
    pub risk_service: Option<Arc<RiskSupervisorService>>,
    pub trading_mode: TradingMode,
    pub armed_exchanges: Vec<String>,
    pub exchange_profile: Option<(String, String)>,
    pub live_state: LivePipelineState,
}

impl ApiContext {
    pub fn new(
        kernel: Arc<Kernel>,
        bus: Arc<dyn MessageBus>,
        log_service: Option<Arc<LogService>>,
        exec_service: Option<Arc<ExecutionService>>,
        cluster_service: Option<Arc<ClusterService>>,
        backtest: Option<Arc<BacktestCoordinator>>,
        market_subjects: Vec<String>,
        net_profile: NetProfile,
        risk_service: Option<Arc<RiskSupervisorService>>,
        trading_mode: TradingMode,
        armed_exchanges: Vec<String>,
        exchange_profile: Option<(String, String)>,
        subscribed_symbols: Vec<String>,
    ) -> Self {
        let live_state = LivePipelineState::new(subscribed_symbols);
        live_state.start_listeners(Arc::clone(&bus), market_subjects.clone());

        Self {
            kernel,
            bus,
            log_service,
            exec_service,
            cluster_service,
            backtest,
            market_subjects,
            net_profile,
            risk_service,
            trading_mode,
            armed_exchanges,
            exchange_profile,
            live_state,
        }
    }

    pub async fn get_node_status(&self) -> Result<NodeStatusDto> {
        let state = self.kernel.state().await;
        let services = self.kernel.service_states().await;
        let services = services
            .into_iter()
            .map(|(id, state)| ServiceStatusDto { id, state })
            .collect();

        Ok(NodeStatusDto {
            kernel_state: format!("{:?}", state),
            kernel_mode: format!("{:?}", self.kernel.mode()),
            trading_mode: format!("{:?}", self.trading_mode),
            armed_exchanges: self.armed_exchanges.clone(),
            services,
        })
    }

    pub async fn health_check(&self) -> Result<bool> {
        let state = self.kernel.state().await;
        let services = self.kernel.service_states().await;
        let services_running = services.values().all(|s| s == "Running");
        Ok(state == KernelState::Running && services_running)
    }

    pub async fn get_recent_logs(&self, limit: usize) -> Result<Vec<LogDto>> {
        if let Some(log_service) = &self.log_service {
            let events: Vec<LogEvent> = log_service.recent_events(limit).await;
            Ok(events.into_iter().map(LogDto::from).collect())
        } else {
            Ok(Vec::new())
        }
    }

    pub async fn get_latency_summary(&self) -> Result<LatencySummaryDto> {
        if let Some(log_service) = &self.log_service {
            Ok(log_service.latency_summary().await.into())
        } else {
            Ok(LatencySummaryDto::default())
        }
    }

    pub async fn get_latency_histogram(&self) -> Result<Vec<LatencyBucketDto>> {
        if let Some(log_service) = &self.log_service {
            let buckets = log_service
                .latency_histogram()
                .await
                .into_iter()
                .map(LatencyBucketDto::from)
                .collect();
            Ok(buckets)
        } else {
            Ok(Vec::new())
        }
    }

    pub async fn get_positions(&self) -> Result<Vec<PositionDto>> {
        if let Some(exec) = &self.exec_service {
            let (balance, positions) = exec.positions().await;
            let mut out = Vec::new();
            for (symbol, size) in positions {
                let base = symbol
                    .chars()
                    .take_while(|c| c.is_alphabetic())
                    .collect::<String>();
                let quote = symbol
                    .chars()
                    .skip_while(|c| c.is_alphabetic())
                    .collect::<String>();
                out.push(PositionDto {
                    symbol: symbol.clone(),
                    base: base.clone(),
                    quote,
                    position_base: size,
                    balance_usdt: balance,
                });
            }
            Ok(out)
        } else {
            Ok(Vec::new())
        }
    }

    pub async fn get_cluster_nodes(&self) -> Result<Vec<ClusterNodeDto>> {
        if let Some(cluster) = &self.cluster_service {
            let nodes = cluster.nodes().await;
            Ok(nodes.into_values().map(ClusterNodeDto::from).collect())
        } else {
            Ok(Vec::new())
        }
    }

    pub async fn start_backtest(
        &self,
        exchange: ExchangeId,
        symbols: Vec<Symbol>,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
        speed_factor: f64,
    ) -> Result<()> {
        if let Some(manager) = &self.backtest {
            let config = BacktestConfig {
                exchange,
                symbols,
                start,
                end,
                speed_factor,
            };
            manager.start(config).await
        } else {
            anyhow::bail!("backtest coordinator not configured")
        }
    }

    pub async fn backtest_status(&self) -> Result<w2uos_backtest::BacktestStatus> {
        if let Some(manager) = &self.backtest {
            Ok(manager.status().await)
        } else {
            anyhow::bail!("backtest coordinator not configured")
        }
    }

    pub async fn risk_status(&self) -> Result<crate::types::RiskStatusDto> {
        if let Some(risk) = &self.risk_service {
            Ok(risk.status().into())
        } else {
            anyhow::bail!("risk supervisor not configured")
        }
    }

    pub async fn reset_circuit(&self) -> Result<()> {
        if let Some(risk) = &self.risk_service {
            risk.reset_circuit().await
        } else {
            anyhow::bail!("risk supervisor not configured")
        }
    }

    pub fn get_net_profile(&self) -> NetProfile {
        self.net_profile.clone()
    }

    pub async fn live_status(&self) -> Result<LiveStatusDto> {
        let last_snapshot = self.live_state.last_snapshot().await;
        let last_ts = last_snapshot.as_ref().map(|snap| snap.ts);
        let connected = last_ts
            .map(|ts| (chrono::Utc::now() - ts) < chrono::Duration::seconds(30))
            .unwrap_or(false);
        let (exchange_name, exchange_mode) = self
            .exchange_profile
            .clone()
            .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));

        Ok(LiveStatusDto {
            exchange: exchange_name,
            mode: exchange_mode,
            symbols: self.live_state.subscribed_symbols().to_vec(),
            market_connected: connected,
            last_snapshot_ts: last_ts,
        })
    }

    pub async fn live_orders(&self, limit: usize) -> Result<Vec<LiveOrderDto>> {
        Ok(self.live_state.recent_trades(limit).await)
    }

    pub async fn live_positions(&self) -> Result<Vec<PositionDto>> {
        self.get_positions().await
    }
}
