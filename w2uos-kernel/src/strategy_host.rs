use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{info, warn};
use w2uos_bus::{BusMessage, MessageBus};
use w2uos_data::{ExchangeId, MarketSnapshot, Symbol};
use w2uos_exec::OrderCommand;
use w2uos_log::{LogEvent, LogLevel, LogSource};
use w2uos_service::TraceId;
use wru_strategy::{BotConfig, PairId, StrategyOutcome, StrategyRuntime};

use crate::{Service, ServiceId};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StrategySubscription {
    pub exchange: ExchangeId,
    pub symbol: Symbol,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StrategyHostConfig {
    pub subscriptions: Vec<StrategySubscription>,
    pub bot_config: BotConfig,
}

impl From<&PairId> for StrategySubscription {
    fn from(pair: &PairId) -> Self {
        Self {
            exchange: pair.exchange.clone(),
            symbol: pair.symbol.clone(),
        }
    }
}

pub trait StrategyEngine: Send {
    fn on_market_snapshot(&mut self, pair: &PairId, snap: &MarketSnapshot) -> StrategyOutcome;
}

impl StrategyEngine for StrategyRuntime {
    fn on_market_snapshot(&mut self, pair: &PairId, snap: &MarketSnapshot) -> StrategyOutcome {
        StrategyRuntime::on_market_snapshot(self, pair, snap)
    }
}

#[derive(Clone)]
pub enum StrategyBackend {
    RustNative(Arc<Mutex<Box<dyn StrategyEngine>>>),
    #[allow(dead_code)]
    Wasm(Arc<Mutex<Box<dyn StrategyEngine>>>),
}

impl StrategyBackend {
    pub fn from_runtime(runtime: StrategyRuntime) -> Self {
        Self::RustNative(Arc::new(Mutex::new(Box::new(runtime))))
    }

    fn engine(&self) -> Arc<Mutex<Box<dyn StrategyEngine>>> {
        match self {
            StrategyBackend::RustNative(engine) | StrategyBackend::Wasm(engine) => {
                Arc::clone(engine)
            }
        }
    }
}

pub struct StrategyHostService {
    id: ServiceId,
    bus: Arc<dyn MessageBus>,
    backend: StrategyBackend,
    config: Arc<RwLock<StrategyHostConfig>>,
    task_handle: Mutex<Option<JoinHandle<()>>>,
    paused: Arc<AtomicBool>,
}

impl StrategyHostService {
    pub fn new(
        id: ServiceId,
        bus: Arc<dyn MessageBus>,
        backend: StrategyBackend,
        config: StrategyHostConfig,
    ) -> Self {
        Self {
            id,
            bus,
            backend,
            config: Arc::new(RwLock::new(config)),
            task_handle: Mutex::new(None),
            paused: Arc::new(AtomicBool::new(false)),
        }
    }

    async fn handle_snapshot(
        bus: Arc<dyn MessageBus>,
        backend: StrategyBackend,
        pair: PairId,
        snapshot: MarketSnapshot,
        paused: Arc<AtomicBool>,
    ) {
        if paused.load(Ordering::SeqCst) {
            return;
        }

        let engine = backend.engine();
        let outcome = {
            let mut guard = engine.lock().expect("strategy runtime lock poisoned");
            guard.on_market_snapshot(&pair, &snapshot)
        };

        let trace_id = snapshot.trace_id.clone().unwrap_or_else(TraceId::new);
        let mut outcome = outcome;
        if outcome.trace_id.is_none() {
            outcome.trace_id = Some(trace_id.clone());
        }

        for (idx, planned) in outcome.planned_orders.iter().enumerate() {
            let cmd = OrderCommand {
                id: format!("{}-{}", snapshot.ts.timestamp_millis(), idx),
                exchange: pair.exchange.clone(),
                symbol: pair.symbol.clone(),
                side: planned.side.clone(),
                order_type: planned.order_type.clone(),
                size_quote: planned.size_quote,
                limit_price: planned.limit_price,
                trace_id: Some(trace_id.clone()),
            };

            if let Ok(payload) = serde_json::to_vec(&cmd) {
                if let Err(err) = bus.publish("orders.command", BusMessage(payload)).await {
                    warn!(?err, "failed to publish order command");
                }
            }
        }

        if let Ok(event_payload) = serde_json::to_vec(&LogEvent {
            ts: chrono::Utc::now(),
            level: LogLevel::Info,
            source: LogSource::Strategy,
            message: "strategy decision".to_string(),
            fields: serde_json::json!({
                "pair": format!("{}.{}{}", pair.exchange, pair.symbol.base, pair.symbol.quote),
                "planned_orders": outcome.planned_orders.len(),
                "notes": outcome.notes,
                "stage": "strategy_decision",
            }),
            correlation_id: None,
            trace_id: Some(trace_id),
        }) {
            if let Err(err) = bus.publish("log.event", BusMessage(event_payload)).await {
                warn!(?err, "failed to publish strategy log event");
            }
        }
    }
}

#[async_trait::async_trait]
impl Service for StrategyHostService {
    fn id(&self) -> &ServiceId {
        &self.id
    }

    async fn start(&self) -> Result<()> {
        let bus = Arc::clone(&self.bus);
        let backend = self.backend.clone();
        let config_handle = Arc::clone(&self.config);
        let subscriptions = self.config.read().await.subscriptions.clone();
        let paused = Arc::clone(&self.paused);

        let bus_for_config = Arc::clone(&self.bus);
        tokio::spawn(async move {
            if let Ok(mut sub) = bus_for_config
                .subscribe("control.config.update.strategy")
                .await
            {
                while let Some(msg) = sub.receiver.recv().await {
                    if let Ok(bot_config) = serde_json::from_slice::<BotConfig>(&msg.0) {
                        let mut guard = config_handle.write().await;
                        guard.bot_config = bot_config.clone();
                        guard.subscriptions = bot_config
                            .pairs
                            .iter()
                            .map(StrategySubscription::from)
                            .collect();
                    }
                }
            }
        });

        let handle = tokio::spawn(async move {
            for sub in subscriptions {
                let subject = format!(
                    "market.{}.{}{}",
                    sub.exchange,
                    sub.symbol.base.to_uppercase(),
                    sub.symbol.quote.to_uppercase()
                );

                let pair = PairId {
                    exchange: sub.exchange.clone(),
                    symbol: sub.symbol.clone(),
                };

                match bus.subscribe(&subject).await {
                    Ok(mut subscription) => {
                        let bus_clone = Arc::clone(&bus);
                        let backend_clone = backend.clone();
                        let paused_clone = Arc::clone(&paused);
                        tokio::spawn(async move {
                            while let Some(msg) = subscription.receiver.recv().await {
                                match serde_json::from_slice::<MarketSnapshot>(&msg.0) {
                                    Ok(snapshot) => {
                                        StrategyHostService::handle_snapshot(
                                            Arc::clone(&bus_clone),
                                            backend_clone.clone(),
                                            pair.clone(),
                                            snapshot,
                                            Arc::clone(&paused_clone),
                                        )
                                        .await;
                                    }
                                    Err(err) => warn!(?err, "failed to deserialize snapshot"),
                                }
                            }
                        });
                    }
                    Err(err) => {
                        warn!(?err, subject = %subject, "failed to subscribe to market feed")
                    }
                }
            }
        });

        let pause_flag = Arc::clone(&self.paused);
        let bus_for_control = Arc::clone(&self.bus);
        tokio::spawn(async move {
            if let Ok(mut sub) = bus_for_control
                .subscribe("control.strategy.pause_all")
                .await
            {
                while let Some(_msg) = sub.receiver.recv().await {
                    pause_flag.store(true, Ordering::SeqCst);
                    warn!("strategy host paused due to circuit breaker");
                }
            }
        });

        let resume_flag = Arc::clone(&self.paused);
        let bus_for_resume = Arc::clone(&self.bus);
        tokio::spawn(async move {
            if let Ok(mut sub) = bus_for_resume
                .subscribe("control.strategy.resume_all")
                .await
            {
                while let Some(_msg) = sub.receiver.recv().await {
                    resume_flag.store(false, Ordering::SeqCst);
                    warn!("strategy host resumed after circuit reset");
                }
            }
        });

        let mut guard = self.task_handle.lock().unwrap();
        *guard = Some(handle);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("strategy host stop requested");
        let mut guard = self.task_handle.lock().unwrap();
        if let Some(handle) = guard.take() {
            handle.abort();
        }
        Ok(())
    }

    async fn health_check(&self) -> Result<()> {
        if self.task_handle.lock().unwrap().is_some() {
            Ok(())
        } else {
            anyhow::bail!("strategy host not running")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{timeout, Duration};
    use w2uos_bus::{BusMessage, LocalBus};
    use w2uos_exec::{OrderSide, OrderType};

    struct FakeRuntime;

    impl StrategyEngine for FakeRuntime {
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
    async fn publishes_order_command_on_snapshot() {
        let bus: Arc<dyn MessageBus> = Arc::new(LocalBus::new());
        let backend = StrategyBackend::RustNative(Arc::new(Mutex::new(Box::new(FakeRuntime))));
        let config = StrategyHostConfig {
            subscriptions: vec![StrategySubscription {
                exchange: ExchangeId::Okx,
                symbol: Symbol {
                    base: "BTC".to_string(),
                    quote: "USDT".to_string(),
                },
            }],
            bot_config: BotConfig::default(),
        };

        let service =
            StrategyHostService::new("strategy".to_string(), Arc::clone(&bus), backend, config);

        let mut order_sub = bus.subscribe("orders.command").await.unwrap();
        service.start().await.unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        let snapshot = MarketSnapshot {
            ts: chrono::Utc::now(),
            exchange: ExchangeId::Okx,
            symbol: Symbol {
                base: "BTC".to_string(),
                quote: "USDT".to_string(),
            },
            last: 100.0,
            bid: 99.5,
            ask: 100.5,
            volume_24h: 1_000.0,
            trace_id: None,
        };

        let subject = "market.OKX.BTCUSDT";
        let payload = serde_json::to_vec(&snapshot).unwrap();
        bus.publish(subject, BusMessage(payload)).await.unwrap();

        let msg = timeout(Duration::from_secs(2), order_sub.receiver.recv())
            .await
            .expect("order command not received")
            .expect("order command payload missing");
        let order: OrderCommand = serde_json::from_slice(&msg.0).unwrap();
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.order_type, OrderType::Market);
    }
}
