use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use w2uos_bus::{BusMessage, MessageBus};
use w2uos_data::Symbol;
use w2uos_log::{
    log_event_via_bus, log_trade_via_bus, LogEvent, LogLevel, LogSource, TradeRecord, TradeSide,
    TradeStatus, TradeSymbol,
};
use w2uos_net::{build_client_and_shaper, NetMode, NetProfile, TrafficShaper};
use w2uos_service::{Service, ServiceId};

use crate::okx::{OkxClient, OkxExecutionConfig, OkxPrivateStream};
use crate::types::{OrderCommand, OrderResult, OrderSide, OrderStatus};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecutionMode {
    Simulated,
    LiveOkx,
}

impl Default for ExecutionMode {
    fn default() -> Self {
        ExecutionMode::Simulated
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExecutionConfig {
    pub initial_balance_usdt: f64,
    pub net_profile: NetProfile,
    #[serde(default)]
    pub live_trading_enabled: bool,
    #[serde(default)]
    pub mode: ExecutionMode,
    pub okx: Option<OkxExecutionConfig>,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            initial_balance_usdt: 100_000.0,
            net_profile: NetProfile::default(),
            live_trading_enabled: false,
            mode: ExecutionMode::Simulated,
            okx: None,
        }
    }
}

#[derive(Debug)]
struct ExecutionState {
    balance_usdt: f64,
    positions: HashMap<String, f64>,
}

pub struct ExecutionService {
    pub bus: Arc<dyn MessageBus>,
    pub config: Arc<RwLock<ExecutionConfig>>,
    state: Mutex<ExecutionState>,
    fill_counter: Mutex<f64>,
    pub http_client: RwLock<Client>,
    pub traffic_shaper: RwLock<TrafficShaper>,
    block_new_orders: Arc<AtomicBool>,
    okx_client: RwLock<Option<OkxClient>>,
}

impl ExecutionService {
    pub fn new(bus: Arc<dyn MessageBus>, config: ExecutionConfig) -> Result<Self> {
        let state = ExecutionState {
            balance_usdt: config.initial_balance_usdt,
            positions: HashMap::new(),
        };

        let (http_client, traffic_shaper) = build_client_and_shaper(&config.net_profile)?;

        let okx_client = if config.live_trading_enabled {
            if let Some(okx_cfg) = &config.okx {
                Some(OkxClient::new(
                    okx_cfg.rest_base_url.clone(),
                    okx_cfg.credentials.clone(),
                    Arc::new(http_client.clone()),
                    Arc::new(traffic_shaper.clone()),
                ))
            } else {
                None
            }
        } else {
            None
        };

        Ok(Self {
            bus,
            config: Arc::new(RwLock::new(config)),
            state: Mutex::new(state),
            fill_counter: Mutex::new(30_000.0),
            http_client: RwLock::new(http_client),
            traffic_shaper: RwLock::new(traffic_shaper),
            block_new_orders: Arc::new(AtomicBool::new(false)),
            okx_client: RwLock::new(okx_client),
        })
    }

    fn symbol_key(symbol: &Symbol) -> String {
        format!("{}{}", symbol.base, symbol.quote).to_uppercase()
    }

    async fn next_fill_price(&self, limit: Option<f64>) -> f64 {
        if let Some(price) = limit {
            return price;
        }

        let mut guard = self.fill_counter.lock().await;
        *guard += 10.0;
        *guard
    }

    async fn handle_command(&self, cmd: OrderCommand) -> Result<OrderResult> {
        if self.block_new_orders.load(Ordering::SeqCst) {
            return Ok(OrderResult {
                command_id: cmd.id,
                exchange: cmd.exchange,
                symbol: cmd.symbol,
                side: cmd.side,
                status: OrderStatus::Rejected("circuit breaker active".to_string()),
                filled_size_quote: 0.0,
                avg_price: 0.0,
                ts: chrono::Utc::now(),
                trace_id: cmd.trace_id,
            });
        }

        if matches!(self.config.read().await.mode, ExecutionMode::LiveOkx) {
            let client_guard = self.okx_client.read().await;
            if let Some(client) = client_guard.as_ref() {
                let resp = client.place_order(&cmd).await?;
                let ord = resp.data.first();
                let status = match ord.and_then(|d| d.s_code.as_deref()) {
                    Some("0") | None => OrderStatus::New,
                    Some(code) => OrderStatus::Rejected(
                        ord.and_then(|d| d.s_msg.clone())
                            .unwrap_or_else(|| code.to_string()),
                    ),
                };

                return Ok(OrderResult {
                    command_id: cmd.id,
                    exchange: cmd.exchange,
                    symbol: cmd.symbol,
                    side: cmd.side,
                    status,
                    filled_size_quote: 0.0,
                    avg_price: 0.0,
                    ts: chrono::Utc::now(),
                    trace_id: cmd.trace_id,
                });
            }
        }

        let fill_price = self.next_fill_price(cmd.limit_price).await;
        let mut state = self.state.lock().await;
        let symbol_key = Self::symbol_key(&cmd.symbol);
        let base_units = if fill_price > 0.0 {
            cmd.size_quote / fill_price
        } else {
            0.0
        };

        let status = match cmd.side {
            OrderSide::Buy => {
                if state.balance_usdt >= cmd.size_quote {
                    state.balance_usdt -= cmd.size_quote;
                    *state.positions.entry(symbol_key).or_default() += base_units;
                    OrderStatus::Filled
                } else {
                    OrderStatus::Rejected("insufficient balance".to_string())
                }
            }
            OrderSide::Sell => {
                let entry = state.positions.entry(symbol_key).or_default();
                if *entry >= base_units {
                    *entry -= base_units;
                    state.balance_usdt += cmd.size_quote;
                    OrderStatus::Filled
                } else {
                    OrderStatus::Rejected("insufficient position".to_string())
                }
            }
        };

        let filled_size = match status {
            OrderStatus::Filled => cmd.size_quote,
            _ => 0.0,
        };

        let filled = matches!(status, OrderStatus::Filled);

        Ok(OrderResult {
            command_id: cmd.id,
            exchange: cmd.exchange,
            symbol: cmd.symbol,
            side: cmd.side,
            status,
            filled_size_quote: if filled { filled_size } else { 0.0 },
            avg_price: if filled { fill_price } else { 0.0 },
            ts: chrono::Utc::now(),
            trace_id: cmd.trace_id.clone(),
        })
    }

    async fn config_snapshot(&self) -> ExecutionConfig {
        self.config.read().await.clone()
    }

    pub async fn run(self: Arc<Self>) -> Result<()> {
        info!("execution service running");
        let cfg = self.config_snapshot().await;
        if matches!(cfg.net_profile.mode, NetMode::Socks5Proxy | NetMode::Tor) {
            info!(mode = %cfg.net_profile.mode, "execution service using proxied network mode");
        }

        if matches!(cfg.mode, ExecutionMode::LiveOkx) {
            info!("execution running in live OKX mode");
            if let Some(okx_cfg) = cfg.okx.clone() {
                let bus = Arc::clone(&self.bus);
                let stream = OkxPrivateStream {
                    bus,
                    ws_url: okx_cfg.ws_private_url,
                    credentials: okx_cfg.credentials,
                };
                tokio::spawn(async move {
                    if let Err(err) = stream.run().await {
                        error!(?err, "okx private stream terminated");
                    }
                });
            }
        }

        let block_flag = Arc::clone(&self.block_new_orders);
        let bus_for_block = Arc::clone(&self.bus);
        tokio::spawn(async move {
            if let Ok(mut sub) = bus_for_block
                .subscribe("control.exec.block_new_orders")
                .await
            {
                while let Some(_msg) = sub.receiver.recv().await {
                    block_flag.store(true, Ordering::SeqCst);
                    info!("execution service circuit breaker activated");
                }
            }
        });

        let block_flag = Arc::clone(&self.block_new_orders);
        let bus_for_reset = Arc::clone(&self.bus);
        tokio::spawn(async move {
            if let Ok(mut sub) = bus_for_reset.subscribe("control.exec.reset_circuit").await {
                while let Some(_msg) = sub.receiver.recv().await {
                    block_flag.store(false, Ordering::SeqCst);
                    info!("execution service circuit breaker reset");
                }
            }
        });
        let start_event = LogEvent {
            ts: chrono::Utc::now(),
            level: LogLevel::Info,
            source: LogSource::Execution,
            message: "execution service started".to_string(),
            fields: serde_json::json!({"initial_balance_usdt": cfg.initial_balance_usdt}),
            correlation_id: None,
            trace_id: None,
        };

        if let Err(err) = log_event_via_bus(self.bus.as_ref(), &start_event).await {
            error!(?err, "failed to publish execution start event");
        }

        let mut sub = self.bus.subscribe("orders.command").await?;
        let mut config_sub = self
            .bus
            .subscribe("control.config.update.execution")
            .await?;

        loop {
            tokio::select! {
                Some(msg) = sub.receiver.recv() => {
                    match serde_json::from_slice::<OrderCommand>(&msg.0) {
                        Ok(cmd) => {
                            info!(id = %cmd.id, ?cmd.side, "received order command");
                            let cmd_for_record = cmd.clone();
                            match self.handle_command(cmd).await {
                                Ok(result) => {
                                    let payload = serde_json::to_vec(&result)?;
                                    if let Err(err) =
                                        self.bus.publish("orders.result", BusMessage(payload)).await
                                    {
                                        error!(?err, "failed to publish order result");
                                    }

                                    let result_event = LogEvent {
                                        ts: result.ts,
                                        level: LogLevel::Info,
                                        source: LogSource::Execution,
                                        message: "order result".to_string(),
                                        fields: serde_json::json!({"stage": "order_result", "status": format!("{:?}", result.status)}),
                                        correlation_id: Some(cmd_for_record.id.clone()),
                                        trace_id: result.trace_id.clone(),
                                    };
                                    if let Err(err) =
                                        log_event_via_bus(self.bus.as_ref(), &result_event).await
                                    {
                                        warn!(?err, "failed to publish order result log event");
                                    }

                                    let trade = TradeRecord {
                                        id: result.command_id.clone(),
                                        ts: result.ts,
                                        exchange: cmd_for_record.exchange.to_string(),
                                        symbol: TradeSymbol {
                                            base: cmd_for_record.symbol.base.clone(),
                                            quote: cmd_for_record.symbol.quote.clone(),
                                        },
                                        side: match cmd_for_record.side {
                                            OrderSide::Buy => TradeSide::Buy,
                                            OrderSide::Sell => TradeSide::Sell,
                                        },
                                        size_quote: result.filled_size_quote,
                                        price: result.avg_price,
                                        status: match &result.status {
                                            OrderStatus::Filled => TradeStatus::Filled,
                                            OrderStatus::Rejected(reason) => {
                                                TradeStatus::Rejected(reason.clone())
                                            }
                                            OrderStatus::New => {
                                                TradeStatus::Rejected("not filled".to_string())
                                            }
                                        },
                                        strategy_id: None,
                                        correlation_id: Some(cmd_for_record.id.clone()),
                                    };

                                    if let Err(err) = log_trade_via_bus(self.bus.as_ref(), &trade).await {
                                        warn!(?err, "failed to publish trade record");
                                    }
                                }
                                Err(err) => {
                                    error!(?err, "failed to handle order command");
                                    let evt = LogEvent {
                                        ts: chrono::Utc::now(),
                                        level: LogLevel::Error,
                                        source: LogSource::Execution,
                                        message: "failed to handle order command".to_string(),
                                        fields: serde_json::json!({"error": err.to_string()}),
                                        correlation_id: Some(cmd_for_record.id.clone()),
                                        trace_id: cmd_for_record.trace_id.clone(),
                                    };
                                    let _ = log_event_via_bus(self.bus.as_ref(), &evt).await;
                                }
                            }
                        }
                        Err(err) => {
                            warn!(?err, "failed to parse order command");
                            let evt = LogEvent {
                                ts: chrono::Utc::now(),
                                level: LogLevel::Warn,
                                source: LogSource::Execution,
                                message: "failed to parse order command".to_string(),
                                fields: serde_json::json!({"error": err.to_string()}),
                                correlation_id: None,
                                trace_id: None,
                            };
                            let _ = log_event_via_bus(self.bus.as_ref(), &evt).await;
                        }
                    }
                }
                Some(msg) = config_sub.receiver.recv() => {
                    if let Ok(new_cfg) = serde_json::from_slice::<ExecutionConfig>(&msg.0) {
                        if let Ok((new_client, new_shaper)) =
                            build_client_and_shaper(&new_cfg.net_profile)
                        {
                            let mut guard = self.http_client.write().await;
                            *guard = new_client;
                            let mut shaper_guard = self.traffic_shaper.write().await;
                            *shaper_guard = new_shaper;
                        }
                        let mut okx_guard = self.okx_client.write().await;
                        if new_cfg.live_trading_enabled {
                            if let Some(okx_cfg) = &new_cfg.okx {
                                *okx_guard = Some(OkxClient::new(
                                    okx_cfg.rest_base_url.clone(),
                                    okx_cfg.credentials.clone(),
                                    Arc::new(self.http_client.read().await.clone()),
                                    Arc::new(self.traffic_shaper.read().await.clone()),
                                ));
                            }
                        } else {
                            *okx_guard = None;
                        }
                        let mut guard = self.config.write().await;
                        *guard = new_cfg;
                    }
                }
                else => break,
            }
        }

        Ok(())
    }

    pub async fn positions(&self) -> (f64, HashMap<String, f64>) {
        let state = self.state.lock().await;
        (state.balance_usdt, state.positions.clone())
    }
}

pub struct ExecutionKernelService {
    id: ServiceId,
    svc: Arc<ExecutionService>,
    task_handle: Mutex<Option<JoinHandle<()>>>,
}

impl ExecutionKernelService {
    pub fn new(id: ServiceId, svc: Arc<ExecutionService>) -> Self {
        Self {
            id,
            svc,
            task_handle: Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl Service for ExecutionKernelService {
    fn id(&self) -> &ServiceId {
        &self.id
    }

    async fn start(&self) -> Result<()> {
        let svc = Arc::clone(&self.svc);
        let handle = tokio::spawn(async move {
            if let Err(err) = svc.run().await {
                error!(?err, "execution service terminated with error");
            }
        });

        let mut guard = self.task_handle.lock().await;
        *guard = Some(handle);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!(service = %self.id, "stop requested for execution service (not yet implemented)");
        Ok(())
    }

    async fn health_check(&self) -> Result<()> {
        let guard = self.task_handle.lock().await;
        if guard.is_some() {
            Ok(())
        } else {
            anyhow::bail!("execution service not started")
        }
    }
}
