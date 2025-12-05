use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{info, warn};
use w2uos_bus::{BusMessage, MessageBus};
use w2uos_data::{ExchangeId, HistoricalStore, Symbol};
use w2uos_exec::{OrderResult, OrderStatus};
use w2uos_service::Service;

#[derive(Clone, Debug)]
pub struct BacktestConfig {
    pub exchange: ExchangeId,
    pub symbols: Vec<Symbol>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub speed_factor: f64,
}

#[derive(Clone, Debug, Default)]
pub struct BacktestResult {
    pub pnl: f64,
    pub max_drawdown: f64,
    pub trade_count: usize,
    pub win_rate: f64,
}

#[derive(Clone, Debug, Default)]
pub struct BacktestStatus {
    pub running: bool,
    pub progress: f64,
    pub current_ts: Option<DateTime<Utc>>,
    pub result: Option<BacktestResult>,
}

#[derive(Clone)]
pub struct BacktestEngine {
    store: HistoricalStore,
    bus: Arc<dyn MessageBus>,
    config: BacktestConfig,
    status: Arc<Mutex<BacktestStatus>>,
}

impl BacktestEngine {
    pub fn new(store: HistoricalStore, bus: Arc<dyn MessageBus>, config: BacktestConfig) -> Self {
        Self::with_status(
            store,
            bus,
            config,
            Arc::new(Mutex::new(BacktestStatus::default())),
        )
    }

    pub fn with_status(
        store: HistoricalStore,
        bus: Arc<dyn MessageBus>,
        config: BacktestConfig,
        status: Arc<Mutex<BacktestStatus>>,
    ) -> Self {
        Self {
            store,
            bus,
            config,
            status,
        }
    }

    pub async fn run(&self) -> Result<BacktestResult> {
        let mut all_snaps = Vec::new();
        for symbol in &self.config.symbols {
            let mut snaps = self
                .store
                .load_snapshots(
                    self.config.exchange.clone(),
                    symbol.clone(),
                    self.config.start,
                    self.config.end,
                )
                .await?;
            all_snaps.append(&mut snaps);
        }

        all_snaps.sort_by_key(|s| s.ts);
        let total = all_snaps.len().max(1);
        let metrics = Arc::new(Mutex::new(BacktestMetrics::default()));

        if let Ok(mut sub) = self.bus.subscribe("orders.result").await {
            let metrics_clone = Arc::clone(&metrics);
            tokio::spawn(async move {
                while let Some(msg) = sub.receiver.recv().await {
                    if let Ok(result) = serde_json::from_slice::<OrderResult>(&msg.0) {
                        metrics_clone.lock().await.record(&result);
                    }
                }
            });
        }

        let mut last_ts: Option<DateTime<Utc>> = None;
        for (idx, snap) in all_snaps.into_iter().enumerate() {
            let subject = format!(
                "market.{}.{}{}",
                snap.exchange,
                snap.symbol.base.to_uppercase(),
                snap.symbol.quote.to_uppercase()
            );
            let payload = serde_json::to_vec(&snap)?;
            self.bus.publish(&subject, BusMessage(payload)).await?;

            {
                let mut guard = self.status.lock().await;
                guard.running = true;
                guard.progress = (idx + 1) as f64 / total as f64;
                guard.current_ts = Some(snap.ts);
            }

            if let Some(prev) = last_ts {
                let delta = snap.ts - prev;
                if let Ok(delay) = delta.to_std() {
                    let scaled = delay
                        .div_f64(self.config.speed_factor.max(0.01))
                        .min(Duration::from_millis(250));
                    sleep(scaled).await;
                }
            }
            last_ts = Some(snap.ts);
        }

        let result = metrics.lock().await.to_result();
        {
            let mut guard = self.status.lock().await;
            guard.running = false;
            guard.progress = 1.0;
            guard.result = Some(result.clone());
        }

        info!(trade_count = result.trade_count, "backtest completed");
        Ok(result)
    }

    pub async fn status(&self) -> BacktestStatus {
        self.status.lock().await.clone()
    }
}

#[derive(Default, Clone)]
struct BacktestMetrics {
    pnl: f64,
    trade_count: usize,
    wins: usize,
    equity_peak: f64,
    equity: f64,
}

impl BacktestMetrics {
    fn record(&mut self, result: &OrderResult) {
        if let OrderStatus::Filled = result.status {
            self.trade_count += 1;
            self.pnl += result.filled_size_quote * 0.001;
            self.equity += self.pnl;
            if self.pnl >= 0.0 {
                self.wins += 1;
            }
            self.equity_peak = self.equity_peak.max(self.equity);
        }
    }

    fn to_result(&self) -> BacktestResult {
        let max_drawdown = if self.equity_peak > 0.0 {
            (self.equity_peak - self.equity).max(0.0) / self.equity_peak
        } else {
            0.0
        };
        let win_rate = if self.trade_count > 0 {
            self.wins as f64 / self.trade_count as f64
        } else {
            0.0
        };
        BacktestResult {
            pnl: self.pnl,
            max_drawdown,
            trade_count: self.trade_count,
            win_rate,
        }
    }
}

pub struct BacktestKernelService {
    id: String,
    engine: Arc<BacktestEngine>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl BacktestKernelService {
    pub fn new(id: String, engine: Arc<BacktestEngine>) -> Self {
        Self {
            id,
            engine,
            task: Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl Service for BacktestKernelService {
    fn id(&self) -> &String {
        &self.id
    }

    async fn start(&self) -> Result<()> {
        let engine = Arc::clone(&self.engine);
        let handle = tokio::spawn(async move {
            if let Err(err) = engine.run().await {
                warn!(?err, "backtest engine terminated with error");
            }
        });
        *self.task.lock().await = Some(handle);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        warn!("backtest stop requested but not yet implemented");
        Ok(())
    }

    async fn health_check(&self) -> Result<()> {
        let guard = self.task.lock().await;
        if let Some(handle) = guard.as_ref() {
            if handle.is_finished() {
                anyhow::bail!("backtest engine finished");
            }
            Ok(())
        } else {
            anyhow::bail!("backtest engine not started")
        }
    }
}

#[derive(Clone)]
pub struct BacktestCoordinator {
    store: HistoricalStore,
    bus: Arc<dyn MessageBus>,
    state: Arc<Mutex<BacktestStatus>>,
    handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl BacktestCoordinator {
    pub fn new(store: HistoricalStore, bus: Arc<dyn MessageBus>) -> Self {
        Self {
            store,
            bus,
            state: Arc::new(Mutex::new(BacktestStatus::default())),
            handle: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn start(&self, config: BacktestConfig) -> Result<()> {
        let mut handle_guard = self.handle.lock().await;
        if let Some(handle) = handle_guard.as_ref() {
            if !handle.is_finished() {
                anyhow::bail!("backtest already running");
            }
        }

        let engine = BacktestEngine::with_status(
            self.store.clone(),
            Arc::clone(&self.bus),
            config,
            Arc::clone(&self.state),
        );
        let state = Arc::clone(&self.state);
        let handle = tokio::spawn(async move {
            {
                let mut guard = state.lock().await;
                guard.running = true;
                guard.progress = 0.0;
                guard.result = None;
            }

            match engine.run().await {
                Ok(res) => {
                    let mut guard = state.lock().await;
                    guard.result = Some(res);
                }
                Err(err) => {
                    warn!(?err, "backtest coordinator run failed");
                }
            }
        });

        *handle_guard = Some(handle);
        Ok(())
    }

    pub async fn status(&self) -> BacktestStatus {
        self.state.lock().await.clone()
    }

    pub fn status_handle(&self) -> Arc<Mutex<BacktestStatus>> {
        Arc::clone(&self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use tokio::time::timeout;
    use w2uos_bus::LocalBus;
    use w2uos_data::MarketSnapshot;
    use w2uos_exec::{
        ExecutionConfig, ExecutionKernelService, ExecutionService, OrderSide, OrderType,
    };
    use w2uos_kernel::{
        StrategyBackend, StrategyHostConfig, StrategyHostService, StrategySubscription,
    };
    use wru_strategy::{BotConfig, PairId, StrategyOutcome};

    struct FakeRuntime;

    impl w2uos_kernel::strategy_host::StrategyEngine for FakeRuntime {
        fn on_market_snapshot(&mut self, pair: &PairId, _snap: &MarketSnapshot) -> StrategyOutcome {
            StrategyOutcome {
                planned_orders: vec![wru_strategy::PlannedOrder {
                    side: OrderSide::Buy,
                    order_type: OrderType::Market,
                    size_quote: 10.0,
                    limit_price: None,
                    trace_id: None,
                }],
                notes: Some(format!("buy {}{}", pair.symbol.base, pair.symbol.quote)),
                trace_id: None,
            }
        }
    }

    #[tokio::test]
    async fn replays_snapshots_and_emits_orders() {
        let bus: Arc<dyn MessageBus> = Arc::new(LocalBus::new());
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let conn = format!("sqlite://{}", tmp.path().to_string_lossy());
        let store = HistoricalStore::connect(&conn).await.unwrap();

        let ts = Utc::now();
        let snapshot = MarketSnapshot {
            ts,
            exchange: ExchangeId::Okx,
            symbol: Symbol {
                base: "BTC".into(),
                quote: "USDT".into(),
            },
            last: 100.0,
            bid: 99.5,
            ask: 100.5,
            volume_24h: 1000.0,
            trace_id: None,
        };
        store.insert_snapshot(&snapshot).await.unwrap();

        let backend = StrategyBackend::RustNative(Arc::new(StdMutex::new(Box::new(FakeRuntime))));
        let config = StrategyHostConfig {
            subscriptions: vec![StrategySubscription {
                exchange: ExchangeId::Okx,
                symbol: snapshot.symbol.clone(),
            }],
            bot_config: BotConfig {
                name: "test".into(),
                pairs: vec![PairId {
                    exchange: ExchangeId::Okx,
                    symbol: snapshot.symbol.clone(),
                }],
            },
        };
        let strategy =
            StrategyHostService::new("strategy".into(), Arc::clone(&bus), backend, config);
        strategy.start().await.unwrap();

        let exec_config = ExecutionConfig {
            initial_balance_usdt: 1000.0,
            net_profile: Default::default(),
            ..Default::default()
        };
        let exec_service = Arc::new(ExecutionService::new(Arc::clone(&bus), exec_config).unwrap());
        let exec_kernel = ExecutionKernelService::new("exec".into(), Arc::clone(&exec_service));
        exec_kernel.start().await.unwrap();

        let engine = BacktestEngine::new(
            store,
            Arc::clone(&bus),
            BacktestConfig {
                exchange: ExchangeId::Okx,
                symbols: vec![snapshot.symbol.clone()],
                start: ts - chrono::Duration::seconds(1),
                end: ts + chrono::Duration::seconds(1),
                speed_factor: 10.0,
            },
        );

        let order_result_future = {
            let mut sub = bus.subscribe("orders.result").await.unwrap();
            tokio::spawn(async move { sub.receiver.recv().await })
        };

        let _ = engine.run().await.unwrap();

        let msg = timeout(Duration::from_secs(10), order_result_future)
            .await
            .expect("order result not received")
            .expect("task failed")
            .expect("message missing");
        let _result: OrderResult = serde_json::from_slice(&msg.0).unwrap();
    }
}
