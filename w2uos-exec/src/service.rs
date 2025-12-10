use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLockWriteGuard;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use w2uos_bus::{BusMessage, MessageBus};
use w2uos_data::{ExchangeId, MarketSnapshot, Symbol, TradingMode};
use w2uos_log::{
    log_event_via_bus, log_trade_via_bus, LogEvent, LogLevel, LogSource, TradeRecord, TradeSide,
    TradeStatus, TradeSymbol,
};
use w2uos_net::{build_client_and_shaper, NetMode, NetProfile, TrafficShaper};
use w2uos_service::{Service, ServiceId};

use crate::binance::BinanceClient;
use crate::okx::{OkxClient, OkxExecutionConfig, OkxPrivateStream};
use crate::types::{OrderCommand, OrderResult, OrderSide, OrderStatus};

use crate::binance::BinanceExecutionConfig;

pub type ExecError = anyhow::Error;

#[async_trait::async_trait]
pub trait ExecutionBackend: Send + Sync {
    async fn handle_order(&self, cmd: OrderCommand) -> Result<OrderResult, ExecError>;
    fn name(&self) -> &'static str;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PaperExecutionConfig {
    /// Basis points of slippage to apply on paper fills (e.g., 5 = 0.05%).
    #[serde(default = "PaperExecutionConfig::default_slippage_bps")]
    pub slippage_bps: f64,
}

impl PaperExecutionConfig {
    fn default_slippage_bps() -> f64 {
        5.0
    }
}

impl Default for PaperExecutionConfig {
    fn default() -> Self {
        Self {
            slippage_bps: Self::default_slippage_bps(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExecutionConfig {
    pub initial_balance_usdt: f64,
    pub net_profile: NetProfile,
    #[serde(default)]
    pub live_trading_enabled: bool,
    #[serde(default)]
    pub mode: TradingMode,
    #[serde(default)]
    pub paper_trading: bool,
    #[serde(default)]
    pub paper: Option<PaperExecutionConfig>,
    pub okx: Option<OkxExecutionConfig>,
    pub binance: Option<BinanceExecutionConfig>,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            initial_balance_usdt: 100_000.0,
            net_profile: NetProfile::default(),
            live_trading_enabled: false,
            mode: TradingMode::Simulated,
            paper_trading: false,
            paper: None,
            okx: None,
            binance: None,
        }
    }
}

#[derive(Debug)]
struct ExecutionState {
    balance_usdt: f64,
    positions: HashMap<String, f64>,
}

struct SimulationBackend {
    state: Arc<Mutex<ExecutionState>>,
    fill_counter: Arc<Mutex<f64>>,
}

impl SimulationBackend {
    fn new(state: Arc<Mutex<ExecutionState>>, fill_counter: Arc<Mutex<f64>>) -> Self {
        Self {
            state,
            fill_counter,
        }
    }

    async fn symbol_key(symbol: &Symbol) -> String {
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
}

#[async_trait::async_trait]
impl ExecutionBackend for SimulationBackend {
    async fn handle_order(&self, cmd: OrderCommand) -> Result<OrderResult, ExecError> {
        let fill_price = self.next_fill_price(cmd.limit_price).await;
        let mut state = self.state.lock().await;
        let symbol_key = Self::symbol_key(&cmd.symbol).await;
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

    fn name(&self) -> &'static str {
        "simulation"
    }
}

/// OKX paper-trading backend for LIVE-3 experiments. This backend simulates
/// fills against the latest live market snapshot without touching real capital
/// or calling private OKX endpoints.
struct OkxPaperBackend {
    bus: Arc<dyn MessageBus>,
    slippage_bps: f64,
    snapshots: Arc<RwLock<HashMap<String, MarketSnapshot>>>,
    subscriptions: Arc<Mutex<HashSet<String>>>,
}

impl OkxPaperBackend {
    fn new(bus: Arc<dyn MessageBus>, slippage_bps: f64) -> Self {
        Self {
            bus,
            slippage_bps,
            snapshots: Arc::new(RwLock::new(HashMap::new())),
            subscriptions: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    fn subject(exchange: &ExchangeId, symbol: &Symbol) -> String {
        format!(
            "market.{}.{}{}",
            exchange,
            symbol.base.to_uppercase(),
            symbol.quote.to_uppercase()
        )
    }

    async fn ensure_subscription(&self, subject: &str) {
        let mut guard = self.subscriptions.lock().await;
        if guard.contains(subject) {
            return;
        }

        guard.insert(subject.to_string());
        let bus = Arc::clone(&self.bus);
        let subject_name = subject.to_string();
        let snapshots = Arc::clone(&self.snapshots);
        tokio::spawn(async move {
            match bus.subscribe(&subject_name).await {
                Ok(mut sub) => {
                    while let Some(msg) = sub.receiver.recv().await {
                        match serde_json::from_slice::<MarketSnapshot>(&msg.0) {
                            Ok(snapshot) => {
                                let mut guard = snapshots.write().await;
                                guard.insert(subject_name.clone(), snapshot);
                            }
                            Err(err) => {
                                warn!(?err, subject = %subject_name, "failed to decode market snapshot");
                            }
                        }
                    }
                }
                Err(err) => {
                    warn!(?err, subject = %subject_name, "failed to subscribe to market data")
                }
            }
        });
    }
}

#[async_trait::async_trait]
impl ExecutionBackend for OkxPaperBackend {
    async fn handle_order(&self, cmd: OrderCommand) -> Result<OrderResult, ExecError> {
        let subject = Self::subject(&cmd.exchange, &cmd.symbol);
        self.ensure_subscription(&subject).await;

        let snapshot = {
            let guard = self.snapshots.read().await;
            guard.get(&subject).cloned()
        }
        .ok_or_else(|| anyhow::anyhow!("no market snapshot available for {}", subject))?;

        let slip = self.slippage_bps / 10_000.0;
        let (reference, status_side) = match cmd.side {
            OrderSide::Buy => {
                let ask = if snapshot.ask > 0.0 {
                    snapshot.ask
                } else {
                    snapshot.last
                };
                (ask * (1.0 + slip), "buy")
            }
            OrderSide::Sell => {
                let bid = if snapshot.bid > 0.0 {
                    snapshot.bid
                } else {
                    snapshot.last
                };
                (bid * (1.0 - slip), "sell")
            }
        };

        if reference <= 0.0 {
            return Err(anyhow::anyhow!("no valid price available for {}", subject));
        }

        let filled_size_quote = cmd.size_quote;
        info!(
            id = %cmd.id,
            %status_side,
            price = reference,
            slippage_bps = self.slippage_bps,
            subject = %subject,
            "paper filled order using live snapshot"
        );
        debug!(
            %subject,
            last = snapshot.last,
            bid = snapshot.bid,
            ask = snapshot.ask,
            volume_24h = snapshot.volume_24h,
            "snapshot used for paper fill"
        );

        Ok(OrderResult {
            command_id: cmd.id,
            exchange: cmd.exchange,
            symbol: cmd.symbol,
            side: cmd.side,
            status: OrderStatus::Filled,
            filled_size_quote,
            avg_price: reference,
            ts: chrono::Utc::now(),
            trace_id: cmd.trace_id.clone(),
        })
    }

    fn name(&self) -> &'static str {
        "okx-paper"
    }
}

struct LiveOkxBackend {
    client: Arc<OkxClient>,
}

#[async_trait::async_trait]
impl ExecutionBackend for LiveOkxBackend {
    async fn handle_order(&self, cmd: OrderCommand) -> Result<OrderResult, ExecError> {
        let resp = self.client.place_order(&cmd).await?;
        let ord = resp.data.first();
        let status = match ord.and_then(|d| d.s_code.as_deref()) {
            Some("0") | None => OrderStatus::New,
            Some(code) => OrderStatus::Rejected(
                ord.and_then(|d| d.s_msg.clone())
                    .unwrap_or_else(|| code.to_string()),
            ),
        };

        Ok(OrderResult {
            command_id: cmd.id,
            exchange: cmd.exchange,
            symbol: cmd.symbol,
            side: cmd.side,
            status,
            filled_size_quote: 0.0,
            avg_price: 0.0,
            ts: chrono::Utc::now(),
            trace_id: cmd.trace_id,
        })
    }

    fn name(&self) -> &'static str {
        "okx-live"
    }
}

struct LiveBinanceBackend {
    client: Arc<BinanceClient>,
}

#[async_trait::async_trait]
impl ExecutionBackend for LiveBinanceBackend {
    async fn handle_order(&self, cmd: OrderCommand) -> Result<OrderResult, ExecError> {
        let result = self.client.place_order(&cmd).await?;
        Ok(result)
    }

    fn name(&self) -> &'static str {
        "binance-live"
    }
}

pub struct ExecutionService {
    pub bus: Arc<dyn MessageBus>,
    pub config: Arc<RwLock<ExecutionConfig>>,
    state: Arc<Mutex<ExecutionState>>,
    fill_counter: Arc<Mutex<f64>>,
    pub http_client: RwLock<Client>,
    pub traffic_shaper: RwLock<TrafficShaper>,
    block_new_orders: Arc<AtomicBool>,
    okx_client: RwLock<Option<Arc<OkxClient>>>,
    binance_client: RwLock<Option<Arc<BinanceClient>>>,
    backend: RwLock<Arc<dyn ExecutionBackend>>, // active execution backend
}

impl ExecutionService {
    pub fn new(bus: Arc<dyn MessageBus>, config: ExecutionConfig) -> Result<Self> {
        let state = Arc::new(Mutex::new(ExecutionState {
            balance_usdt: config.initial_balance_usdt,
            positions: HashMap::new(),
        }));

        let (http_client, traffic_shaper) = build_client_and_shaper(&config.net_profile)?;

        let okx_client = if config.live_trading_enabled {
            if let Some(okx_cfg) = &config.okx {
                Some(Arc::new(OkxClient::new(
                    okx_cfg.rest_base_url.clone(),
                    okx_cfg.credentials.clone(),
                    Arc::new(http_client.clone()),
                    Arc::new(traffic_shaper.clone()),
                )))
            } else {
                None
            }
        } else {
            None
        };

        let binance_client = if config.live_trading_enabled {
            if let Some(binance_cfg) = &config.binance {
                Some(Arc::new(BinanceClient::new(
                    binance_cfg.rest_base_url.clone(),
                    binance_cfg.credentials.clone(),
                    Arc::new(http_client.clone()),
                    Arc::new(traffic_shaper.clone()),
                )))
            } else {
                None
            }
        } else {
            None
        };

        let fill_counter = Arc::new(Mutex::new(30_000.0));

        let backend = Self::build_backend(
            Arc::clone(&bus),
            &config,
            Arc::clone(&state),
            Arc::clone(&fill_counter),
            okx_client.clone(),
            binance_client.clone(),
        );

        Ok(Self {
            bus,
            config: Arc::new(RwLock::new(config)),
            state,
            fill_counter,
            http_client: RwLock::new(http_client),
            traffic_shaper: RwLock::new(traffic_shaper),
            block_new_orders: Arc::new(AtomicBool::new(false)),
            okx_client: RwLock::new(okx_client),
            binance_client: RwLock::new(binance_client),
            backend: RwLock::new(backend),
        })
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

        let backend = self.backend.read().await.clone();
        match backend.handle_order(cmd.clone()).await {
            Ok(result) => Ok(result),
            Err(err) => {
                warn!(?err, backend = backend.name(), "backend rejected order");
                Ok(OrderResult {
                    command_id: cmd.id,
                    exchange: cmd.exchange,
                    symbol: cmd.symbol,
                    side: cmd.side,
                    status: OrderStatus::Rejected(err.to_string()),
                    filled_size_quote: 0.0,
                    avg_price: 0.0,
                    ts: chrono::Utc::now(),
                    trace_id: cmd.trace_id.clone(),
                })
            }
        }
    }

    async fn config_snapshot(&self) -> ExecutionConfig {
        self.config.read().await.clone()
    }

    fn build_backend(
        bus: Arc<dyn MessageBus>,
        cfg: &ExecutionConfig,
        state: Arc<Mutex<ExecutionState>>,
        fill_counter: Arc<Mutex<f64>>,
        okx_client: Option<Arc<OkxClient>>,
        binance_client: Option<Arc<BinanceClient>>,
    ) -> Arc<dyn ExecutionBackend> {
        if matches!(cfg.mode, TradingMode::LiveOkx) && cfg.paper_trading {
            let slippage = cfg
                .paper
                .as_ref()
                .map(|p| p.slippage_bps)
                .unwrap_or_else(PaperExecutionConfig::default_slippage_bps);
            return Arc::new(OkxPaperBackend::new(bus, slippage));
        }

        if matches!(cfg.mode, TradingMode::LiveOkx) {
            if let Some(client) = okx_client {
                return Arc::new(LiveOkxBackend { client });
            }
            warn!("live OKX mode requested but client unavailable; falling back to simulation");
        }

        if matches!(cfg.mode, TradingMode::LiveBinance) {
            if let Some(client) = binance_client {
                return Arc::new(LiveBinanceBackend { client });
            }
            warn!("live Binance mode requested but client unavailable; falling back to simulation");
        }

        Arc::new(SimulationBackend::new(state, fill_counter))
    }

    async fn refresh_backend(&self) {
        let cfg = self.config.read().await.clone();
        let okx_client = self.okx_client.read().await.clone();
        let binance_client = self.binance_client.read().await.clone();
        let backend = Self::build_backend(
            Arc::clone(&self.bus),
            &cfg,
            Arc::clone(&self.state),
            Arc::clone(&self.fill_counter),
            okx_client,
            binance_client,
        );

        let mut guard: RwLockWriteGuard<'_, Arc<dyn ExecutionBackend>> = self.backend.write().await;
        *guard = backend;
    }

    pub async fn run(self: Arc<Self>) -> Result<()> {
        info!("execution service running");
        let cfg = self.config_snapshot().await;
        if matches!(cfg.net_profile.mode, NetMode::Socks5Proxy | NetMode::Tor) {
            info!(mode = %cfg.net_profile.mode, "execution service using proxied network mode");
        }

        if matches!(cfg.mode, TradingMode::LiveOkx) {
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
                            let mut binance_guard = self.binance_client.write().await;
                            if new_cfg.live_trading_enabled {
                                if let Some(okx_cfg) = &new_cfg.okx {
                                    *okx_guard = Some(Arc::new(OkxClient::new(
                                        okx_cfg.rest_base_url.clone(),
                                        okx_cfg.credentials.clone(),
                                        Arc::new(self.http_client.read().await.clone()),
                                        Arc::new(self.traffic_shaper.read().await.clone()),
                                    )));
                                }
                                if let Some(binance_cfg) = &new_cfg.binance {
                                    *binance_guard = Some(Arc::new(BinanceClient::new(
                                        binance_cfg.rest_base_url.clone(),
                                        binance_cfg.credentials.clone(),
                                        Arc::new(self.http_client.read().await.clone()),
                                        Arc::new(self.traffic_shaper.read().await.clone()),
                                    )));
                                }
                            } else {
                                *okx_guard = None;
                                *binance_guard = None;
                            }
                            let mut guard = self.config.write().await;
                            *guard = new_cfg;
                            drop(guard);
                            self.refresh_backend().await;
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
