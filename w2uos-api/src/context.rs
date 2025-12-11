use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use chrono::Utc;
use hostname::get;
use sysinfo::{Disks, System};
use uuid::Uuid;
use w2uos_backtest::{BacktestConfig, BacktestCoordinator};
use w2uos_bus::{BusMessage, MessageBus};
use w2uos_data::TradingMode;
use w2uos_data::{ExchangeId, MarketSnapshot, Symbol};
use w2uos_exec::ExecutionService;
use w2uos_kernel::{
    ClusterService, Kernel, KernelState, RiskProfile, RiskSupervisorService, TradingState,
    TradingStateTransition,
};
use w2uos_log::{types::LogEvent, ControlActionRecord, LogService};
use w2uos_net::NetProfile;

use crate::types::{
    ClusterNodeDto, ControlActionDto, ControlResponseDto, HealthStatusDto, IbmqInfoDto,
    LatencyBucketDto, LatencySummaryDto, LiveOrderDto, LiveStatusDto, LogDto, MarketMetricsDto,
    MarketSummaryResponseDto, MarketSymbolSummaryDto, NodeInfoDto, NodeStatusDto,
    OrderbookSummaryDto, PositionDto, RecentTradeDto, RiskInfoDto, RiskProfileResponseDto,
    RuntimeInfoDto, ServiceStatusDto, StrategyInfoDto, SystemMetricsDto, X402InfoDto,
};

#[derive(Clone)]
pub struct LivePipelineState {
    last_snapshot: Arc<tokio::sync::Mutex<Option<MarketSnapshot>>>,
    snapshots_by_symbol: Arc<tokio::sync::Mutex<HashMap<String, MarketSnapshot>>>,
    trades: Arc<tokio::sync::Mutex<Vec<LiveOrderDto>>>,
    symbols: Vec<String>,
}

impl LivePipelineState {
    pub fn new(symbols: Vec<String>) -> Self {
        let symbols = symbols.into_iter().map(|s| normalize_symbol(&s)).collect();
        Self {
            last_snapshot: Arc::new(tokio::sync::Mutex::new(None)),
            snapshots_by_symbol: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            trades: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            symbols,
        }
    }

    pub fn start_listeners(&self, bus: Arc<dyn MessageBus>, subjects: Vec<String>) {
        let last_snapshot = Arc::clone(&self.last_snapshot);
        let snapshots_by_symbol = Arc::clone(&self.snapshots_by_symbol);
        let symbol_filter = self.symbols.clone();
        let bus_for_snapshots = Arc::clone(&bus);
        tokio::spawn(async move {
            for subject in subjects {
                let mut sub = match bus_for_snapshots.subscribe(&subject).await {
                    Ok(sub) => sub,
                    Err(_) => continue,
                };

                let last_snapshot = Arc::clone(&last_snapshot);
                let snapshots_by_symbol = Arc::clone(&snapshots_by_symbol);
                let symbol_filter = symbol_filter.clone();
                tokio::spawn(async move {
                    while let Some(msg) = sub.receiver.recv().await {
                        if let Ok(snapshot) = serde_json::from_slice::<MarketSnapshot>(&msg.0) {
                            let key = format!(
                                "{}-{}",
                                snapshot.symbol.base.to_ascii_uppercase(),
                                snapshot.symbol.quote.to_ascii_uppercase()
                            );

                            if symbol_filter.is_empty()
                                || symbol_filter
                                    .iter()
                                    .any(|sym| sym.eq_ignore_ascii_case(&key))
                            {
                                {
                                    let mut guard = snapshots_by_symbol.lock().await;
                                    guard.insert(key.clone(), snapshot.clone());
                                }

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

    pub async fn last_snapshot_for(&self, symbol: &str) -> Option<MarketSnapshot> {
        let key = normalize_symbol(symbol);
        let guard = self.snapshots_by_symbol.lock().await;
        guard.get(&key).cloned()
    }

    pub async fn latest_snapshot_ts(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        let guard = self.snapshots_by_symbol.lock().await;
        guard
            .values()
            .max_by_key(|snap| snap.ts)
            .map(|snap| snap.ts)
    }

    pub async fn recent_trades(&self, limit: usize) -> Vec<LiveOrderDto> {
        let guard = self.trades.lock().await;
        let len = guard.len();
        let start = len.saturating_sub(limit);
        guard[start..].to_vec()
    }

    pub async fn recent_trades_for(&self, symbol: &str, limit: usize) -> Vec<LiveOrderDto> {
        let key = normalize_symbol(symbol);
        let guard = self.trades.lock().await;
        let filtered: Vec<LiveOrderDto> = guard
            .iter()
            .filter(|t| normalize_symbol(&t.symbol) == key)
            .cloned()
            .collect();
        let len = filtered.len();
        let start = len.saturating_sub(limit);
        filtered[start..].to_vec()
    }

    pub fn subscribed_symbols(&self) -> &[String] {
        &self.symbols
    }

    pub fn known_symbols(&self) -> Vec<String> {
        self.symbols.clone()
    }
}

fn normalize_symbol(symbol: &str) -> String {
    symbol
        .replace('/', "-")
        .to_ascii_uppercase()
        .trim()
        .to_string()
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
    pub node_id: String,
    pub env: String,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub version: String,
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
        node_id: String,
        env: String,
    ) -> Self {
        let live_state = LivePipelineState::new(subscribed_symbols);
        live_state.start_listeners(Arc::clone(&bus), market_subjects.clone());

        let start_time = Utc::now();
        let version = env!("CARGO_PKG_VERSION").to_string();

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
            node_id,
            env,
            start_time,
            version,
        }
    }

    pub async fn get_node_status(&self) -> Result<NodeStatusDto> {
        let state = self.kernel.state().await;
        let trading_state: TradingState = self.kernel.trading_state().await;
        let (risk_profile, risk_limits) = self.kernel.risk_profile().await;
        let services = self.kernel.service_states().await;
        let services = services
            .into_iter()
            .map(|(id, state)| ServiceStatusDto { id, state })
            .collect();

        Ok(NodeStatusDto {
            kernel_state: format!("{:?}", state),
            kernel_mode: format!("{:?}", self.kernel.mode()),
            trading_mode: format!("{:?}", self.trading_mode),
            trading_state: trading_state.as_str().to_string(),
            node_id: self.node_id.clone(),
            env: self.env.clone(),
            armed_exchanges: self.armed_exchanges.clone(),
            risk_profile: risk_profile.as_str().to_string(),
            max_leverage: risk_limits.max_leverage,
            max_notional_usdt: risk_limits.max_notional_usdt,
            max_concurrent_positions: risk_limits.max_concurrent_positions,
            services,
        })
    }

    pub async fn system_metrics(&self) -> Result<SystemMetricsDto> {
        let trading_state: TradingState = self.kernel.trading_state().await;
        let (risk_profile, risk_limits) = self.kernel.risk_profile().await;

        let uptime_seconds = Utc::now()
            .signed_duration_since(self.start_time)
            .num_seconds()
            .max(0) as u64;
        let hostname = get()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown".to_string());

        let mut sys = System::new();
        sys.refresh_cpu();
        sys.refresh_memory();

        let cpu_load_pct = sys.global_cpu_info().cpu_usage() as f64;
        let mem_used_mb = sys.used_memory() as f64 / 1024.0;
        let mem_total_mb = sys.total_memory() as f64 / 1024.0;

        let disks = Disks::new_with_refreshed_list();
        let (disk_used_gb, disk_total_gb) = disks
            .iter()
            .next()
            .map(|disk| {
                let total = disk.total_space() as f64 / 1_073_741_824.0;
                let available = disk.available_space() as f64 / 1_073_741_824.0;
                let used = (total - available).max(0.0);
                (used, total)
            })
            .unwrap_or((0.0, 0.0));

        let node = NodeInfoDto {
            node_id: self.node_id.clone(),
            env: self.env.clone(),
            version: self.version.clone(),
            uptime_seconds,
            hostname,
            trading_state: trading_state.as_str().to_string(),
        };

        let runtime = RuntimeInfoDto {
            cpu_load_pct,
            mem_used_mb,
            mem_total_mb,
            disk_used_gb,
            disk_total_gb,
        };

        let strategy = StrategyInfoDto {
            active_strategies: vec![],
            signals_per_minute: 0.0,
            avg_decision_latency_ms: 0.0,
            error_count_last_10m: 0,
        };

        let risk = RiskInfoDto {
            risk_profile: risk_profile.as_str().to_string(),
            max_leverage: risk_limits.max_leverage,
            max_notional_usdt: risk_limits.max_notional_usdt,
            max_concurrent_positions: risk_limits.max_concurrent_positions,
            current_notional_usdt: 0.0,
            open_positions_count: 0,
            intraday_drawdown_pct: 0.0,
        };

        let ibmq = IbmqInfoDto {
            enabled: false,
            role: "disabled".to_string(),
            last_job_ts: None,
            last_job_status: None,
            last_solution_score: None,
            schedule_mode: None,
        };

        let x402 = X402InfoDto {
            enabled: false,
            role: "disabled".to_string(),
            registered_agents: 0,
            pending_settlements: 0,
            last_settlement_ts: None,
        };

        Ok(SystemMetricsDto {
            node,
            runtime,
            strategy,
            risk,
            ibmq,
            x402,
        })
    }

    pub async fn health_check(&self) -> Result<bool> {
        let state = self.kernel.state().await;
        let services = self.kernel.service_states().await;
        let services_running = services.values().all(|s| s == "Running");
        Ok(state == KernelState::Running && services_running)
    }

    pub async fn health_status(&self) -> Result<HealthStatusDto> {
        let state = self.kernel.state().await;
        let services = self.kernel.service_states().await;

        let market_connected = services
            .get("market-data")
            .map(|s| s == "Running")
            .unwrap_or(false);
        let execution_connected = services
            .get("execution")
            .map(|s| s == "Running")
            .unwrap_or(false);

        Ok(HealthStatusDto {
            kernel_state: format!("{:?}", state),
            market_connected,
            execution_connected,
            ws_public_alive: market_connected,
            ws_private_alive: execution_connected,
        })
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
        let last_ts = self.live_state.latest_snapshot_ts().await;
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

    pub async fn market_metrics(
        &self,
        symbol: &str,
        limit: usize,
    ) -> Result<Option<MarketMetricsDto>> {
        let snapshot = self.live_state.last_snapshot_for(symbol).await;
        let Some(snapshot) = snapshot else {
            return Ok(None);
        };

        let bid = snapshot.bid;
        let ask = snapshot.ask;
        let spread_bp = if bid > 0.0 && ask > 0.0 {
            ((ask - bid) / ((bid + ask) / 2.0)) * 10_000.0
        } else {
            0.0
        };

        let recent_trades: Vec<RecentTradeDto> = self
            .live_state
            .recent_trades_for(symbol, limit)
            .await
            .into_iter()
            .map(|t| RecentTradeDto {
                ts: t.ts.to_rfc3339(),
                side: t.side.to_ascii_lowercase(),
                price: t.price,
                size_quote: t.size_quote,
            })
            .collect();

        let orderbook = OrderbookSummaryDto {
            best_bid_size: 0.0,
            best_ask_size: 0.0,
            bid_depth_usdt: 0.0,
            ask_depth_usdt: 0.0,
        };

        let exchange = format!("{}", snapshot.exchange);

        Ok(Some(MarketMetricsDto {
            symbol: format!("{}/{}", snapshot.symbol.base, snapshot.symbol.quote),
            exchange,
            last_price: snapshot.last,
            bid,
            ask,
            spread_bp,
            volume_24h: snapshot.volume_24h,
            funding_rate: None,
            open_interest: None,
            volatility_1h_pct: None,
            volatility_24h_pct: None,
            orderbook,
            recent_trades,
        }))
    }

    pub async fn market_summary(
        &self,
        exchange: Option<&str>,
        limit: usize,
    ) -> Result<MarketSummaryResponseDto> {
        let exchange_name = exchange
            .map(|e| e.to_string())
            .or_else(|| self.exchange_profile.as_ref().map(|(name, _)| name.clone()))
            .unwrap_or_else(|| "UNKNOWN".to_string());

        let symbols = self.live_state.known_symbols();
        let limit = limit.min(symbols.len());
        let symbols = symbols.into_iter().take(limit).collect::<Vec<_>>();

        let open_positions = self.open_position_symbols().await;

        let mut summaries = Vec::new();
        for sym in symbols {
            let snapshot = self.live_state.last_snapshot_for(&sym).await;
            let (last_price, volume_24h) = snapshot
                .as_ref()
                .map(|s| (s.last, s.volume_24h))
                .unwrap_or((0.0, 0.0));

            summaries.push(MarketSymbolSummaryDto {
                symbol: sym.clone(),
                last_price,
                change_24h_pct: 0.0,
                volatility_24h_pct: 0.0,
                volume_24h,
                in_watchlist: false,
                has_open_position: open_positions.contains(&normalize_symbol(&sym)),
            });
        }

        Ok(MarketSummaryResponseDto {
            exchange: exchange_name,
            symbols: summaries,
        })
    }

    async fn open_position_symbols(&self) -> std::collections::HashSet<String> {
        use std::collections::HashSet;

        if let Some(exec) = &self.exec_service {
            let (_, positions) = exec.positions().await;
            positions
                .keys()
                .map(|s| normalize_symbol(s))
                .collect::<HashSet<String>>()
        } else {
            HashSet::new()
        }
    }

    async fn record_control_action(
        &self,
        action: &str,
        params: serde_json::Value,
        transition: Option<TradingStateTransition>,
        override_states: Option<(Option<String>, Option<String>)>,
        actor: &str,
    ) -> Result<String> {
        let audit_id = Uuid::new_v4().to_string();
        if let Some(log) = &self.log_service {
            let previous_state = override_states
                .as_ref()
                .and_then(|(p, _)| p.clone())
                .or_else(|| transition.as_ref().map(|t| t.previous.as_str().to_string()));
            let new_state = override_states
                .as_ref()
                .and_then(|(_, n)| n.clone())
                .or_else(|| {
                    transition
                        .as_ref()
                        .map(|t| t.new_state.as_str().to_string())
                });
            let record = ControlActionRecord {
                id: None,
                audit_id: audit_id.clone(),
                ts: Utc::now(),
                actor: actor.to_string(),
                action: action.to_string(),
                params,
                previous_state,
                new_state,
            };
            log.log_control_action(record).await?;
        }

        Ok(audit_id)
    }

    pub async fn control_pause(
        &self,
        reason: Option<String>,
        actor: &str,
    ) -> Result<ControlResponseDto> {
        let transition = self.kernel.pause();
        let audit_id = self
            .record_control_action(
                "control/pause",
                serde_json::json!({"reason": reason.clone()}),
                Some(transition),
                None,
                actor,
            )
            .await?;

        Ok(ControlResponseDto::new(
            true,
            TradingState::Paused,
            "Trading state changed to PAUSED",
            audit_id,
        ))
    }

    pub async fn control_resume(
        &self,
        reason: Option<String>,
        actor: &str,
    ) -> Result<ControlResponseDto> {
        let transition = self.kernel.resume()?;
        let new_state = transition.new_state;
        let audit_id = self
            .record_control_action(
                "control/resume",
                serde_json::json!({"reason": reason.clone()}),
                Some(transition.clone()),
                None,
                actor,
            )
            .await?;

        Ok(ControlResponseDto::new(
            true,
            new_state,
            "Trading state resumed",
            audit_id,
        ))
    }

    pub async fn control_freeze(
        &self,
        reason: Option<String>,
        cancel_open_orders: bool,
        actor: &str,
    ) -> Result<ControlResponseDto> {
        let transition = self.kernel.freeze(cancel_open_orders);
        let audit_id = self
            .record_control_action(
                "control/freeze",
                serde_json::json!({"reason": reason.clone(), "cancel_open_orders": cancel_open_orders}),
                Some(transition),
                None,
                actor,
            )
            .await?;

        Ok(ControlResponseDto::new(
            true,
            TradingState::Frozen,
            "Trading state changed to FROZEN",
            audit_id,
        ))
    }

    pub async fn control_unfreeze(
        &self,
        reason: Option<String>,
        target: TradingState,
        actor: &str,
    ) -> Result<ControlResponseDto> {
        let transition = self.kernel.unfreeze(target)?;
        let audit_id = self
            .record_control_action(
                "control/unfreeze",
                serde_json::json!({"reason": reason.clone(), "target_state": target.as_str()}),
                Some(transition),
                None,
                actor,
            )
            .await?;

        Ok(ControlResponseDto::new(
            true,
            target,
            "Trading state changed via unfreeze",
            audit_id,
        ))
    }

    pub async fn control_flatten(
        &self,
        reason: Option<String>,
        mode: Option<String>,
        timeout_ms: Option<u64>,
        actor: &str,
    ) -> Result<ControlResponseDto> {
        let current = self.kernel.trading_state().await;
        self.kernel
            .request_flatten(reason.clone(), mode.clone(), timeout_ms);
        let audit_id = self
            .record_control_action(
                "control/flatten",
                serde_json::json!({"reason": reason, "mode": mode, "timeout_ms": timeout_ms}),
                Some(TradingStateTransition {
                    previous: current,
                    new_state: current,
                }),
                None,
                actor,
            )
            .await?;

        Ok(ControlResponseDto::new(
            true,
            current,
            "Flatten request recorded",
            audit_id,
        ))
    }

    pub async fn control_live_switch(
        &self,
        armed: bool,
        reason: Option<String>,
        actor: &str,
    ) -> Result<crate::types::LiveSwitchResponseDto> {
        let previous_state = self.kernel.live_switch_state().await.0;
        let new_state = self.kernel.set_live_switch(armed, reason.clone());
        let _ = self
            .bus
            .publish(
                "control.live_switch",
                BusMessage(serde_json::to_vec(&new_state)?),
            )
            .await;

        let audit_id = self
            .record_control_action(
                "control/live_switch",
                serde_json::json!({"reason": reason.clone(), "armed": armed}),
                None,
                Some((
                    Some(if previous_state {
                        "ON".to_string()
                    } else {
                        "OFF".to_string()
                    }),
                    Some(if new_state {
                        "ON".to_string()
                    } else {
                        "OFF".to_string()
                    }),
                )),
                actor,
            )
            .await?;

        Ok(crate::types::LiveSwitchResponseDto {
            ok: true,
            armed: new_state,
            message: format!(
                "Live switch set to {}",
                if new_state { "ON" } else { "OFF" }
            ),
            audit_id,
        })
    }

    pub async fn control_state(&self) -> Result<crate::types::ControlStateDto> {
        let snapshot = self.kernel.control_snapshot().await;
        let (risk_profile, _) = self.kernel.risk_profile().await;
        Ok(crate::types::ControlStateDto {
            trading_state: snapshot.trading_state.as_str().to_string(),
            risk_profile: risk_profile.as_str().to_string(),
            live_switch_armed: snapshot.live_switch_armed,
            live_switch_reason: snapshot.live_switch_reason,
        })
    }

    pub async fn control_audit_log(&self, limit: usize) -> Result<Vec<ControlActionDto>> {
        if let Some(log) = &self.log_service {
            let entries = log.recent_control_actions(limit).await;
            Ok(entries.into_iter().map(ControlActionDto::from).collect())
        } else {
            Ok(Vec::new())
        }
    }

    pub async fn control_risk_profile(
        &self,
        profile: RiskProfile,
        reason: Option<String>,
        actor: &str,
    ) -> Result<RiskProfileResponseDto> {
        let transition = self.kernel.set_risk_profile(profile);
        let audit_id = self
            .record_control_action(
                "control/risk-profile",
                serde_json::json!({
                    "reason": reason.clone(),
                    "profile": profile.as_str(),
                }),
                None,
                Some((
                    Some(transition.previous.as_str().to_string()),
                    Some(transition.new_profile.as_str().to_string()),
                )),
                actor,
            )
            .await?;

        Ok(RiskProfileResponseDto {
            ok: true,
            risk_profile: transition.new_profile.as_str().to_string(),
            max_leverage: transition.limits.max_leverage,
            max_notional_usdt: transition.limits.max_notional_usdt,
            max_concurrent_positions: transition.limits.max_concurrent_positions,
            message: format!(
                "Risk profile switched to {}",
                transition.new_profile.as_str()
            ),
            audit_id,
        })
    }

    pub async fn control_mode_switch(
        &self,
        target: TradingState,
        double_confirm_token: Option<String>,
        reason: Option<String>,
        actor: &str,
    ) -> Result<ControlResponseDto> {
        if target == TradingState::LiveArmed
            && double_confirm_token
                .as_ref()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
        {
            anyhow::bail!("double_confirm_token required for LIVE_ARMED");
        }

        let current = self.kernel.trading_state().await;
        let transition = if current == target {
            TradingStateTransition {
                previous: current,
                new_state: target,
            }
        } else {
            self.kernel.switch_trading_state(target)?
        };

        let audit_id = self
            .record_control_action(
                "control/mode",
                serde_json::json!({
                    "reason": reason,
                    "target_mode": target.as_str(),
                    "double_confirm_token": double_confirm_token,
                }),
                Some(transition.clone()),
                None,
                actor,
            )
            .await?;

        Ok(ControlResponseDto::new(
            true,
            transition.new_state,
            &format!("Trading mode switched to {}", transition.new_state.as_str()),
            audit_id,
        ))
    }
}
