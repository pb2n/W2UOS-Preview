use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration, Instant};
use tracing::{error, warn};
use w2uos_bus::{BusMessage, MessageBus};
use w2uos_exec::{OrderResult, OrderSide};
use w2uos_log::{LogEvent, LogLevel, LogSource};

use crate::config_manager::SUBJECT_RISK_UPDATE;
use crate::risk_config::GlobalRiskConfig;
use crate::{Service, ServiceId};

#[derive(Clone, Debug, Default)]
pub struct RiskStatus {
    pub daily_pnl_quote: f64,
    pub total_notional: f64,
    pub open_positions: usize,
    pub circuit_breaker_active: bool,
}

#[derive(Clone)]
struct RiskState {
    status: RiskStatus,
    last_errors: VecDeque<Instant>,
    exposure: HashMap<String, f64>,
}

pub struct RiskSupervisorService {
    id: ServiceId,
    config: Arc<RwLock<GlobalRiskConfig>>,
    bus: Arc<dyn MessageBus>,
    state: Mutex<RiskState>,
    task_handle: Mutex<Option<JoinHandle<()>>>,
}

impl RiskSupervisorService {
    pub fn new(id: ServiceId, config: GlobalRiskConfig, bus: Arc<dyn MessageBus>) -> Self {
        let state = RiskState {
            status: RiskStatus::default(),
            last_errors: VecDeque::new(),
            exposure: HashMap::new(),
        };

        Self {
            id,
            config: Arc::new(RwLock::new(config)),
            bus,
            state: Mutex::new(state),
            task_handle: Mutex::new(None),
        }
    }

    fn record_order(&self, result: &OrderResult) {
        let mut guard = self.state.lock().unwrap();
        if let w2uos_exec::OrderStatus::Filled = result.status {
            let symbol_key =
                format!("{}{}", result.symbol.base, result.symbol.quote).to_uppercase();
            let entry = guard.exposure.entry(symbol_key.clone()).or_insert(0.0);
            let pnl_delta = match result.side {
                OrderSide::Buy => {
                    *entry += result.filled_size_quote;
                    -result.filled_size_quote
                }
                OrderSide::Sell => {
                    *entry -= result.filled_size_quote;
                    result.filled_size_quote
                }
            };
            let exposure_value = *entry;

            if exposure_value <= 0.0 {
                guard.exposure.remove(&symbol_key);
            }

            guard.status.daily_pnl_quote += pnl_delta;
            guard.status.total_notional = guard.exposure.values().copied().sum();
            guard.status.open_positions = guard.exposure.len();
        }
    }

    fn record_error(&self) {
        let mut guard = self.state.lock().unwrap();
        guard.last_errors.push_back(Instant::now());
    }

    fn prune_errors(&self) -> f64 {
        let mut guard = self.state.lock().unwrap();
        let cutoff = Instant::now() - Duration::from_secs(60);
        while let Some(front) = guard.last_errors.front() {
            if *front < cutoff {
                guard.last_errors.pop_front();
            } else {
                break;
            }
        }
        guard.last_errors.len() as f64
    }

    fn circuit_breaker_triggered(&self) -> bool {
        self.state.lock().unwrap().status.circuit_breaker_active
    }

    fn set_circuit_breaker(&self) {
        let mut guard = self.state.lock().unwrap();
        guard.status.circuit_breaker_active = true;
    }

    pub async fn reset_circuit(&self) -> Result<()> {
        let mut guard = self.state.lock().unwrap();
        guard.status.circuit_breaker_active = false;
        self.bus
            .publish("control.exec.reset_circuit", BusMessage(Vec::new()))
            .await?;
        self.bus
            .publish("control.strategy.resume_all", BusMessage(Vec::new()))
            .await?;
        Ok(())
    }

    pub fn status(&self) -> RiskStatus {
        self.state.lock().unwrap().status.clone()
    }

    async fn evaluate(&self) {
        let status = self.status();
        if status.circuit_breaker_active {
            return;
        }

        let config = self.config.read().await.clone();

        if status.daily_pnl_quote <= -config.max_daily_loss_abs
            || status.total_notional > config.max_total_notional
            || status.open_positions > config.max_open_positions
        {
            self.trigger_breaker("risk limits breached").await;
            return;
        }

        if let Some(max_rate) = config.max_error_rate_per_minute {
            let error_rate = self.prune_errors();
            if error_rate > max_rate {
                self.trigger_breaker("error rate exceeded").await;
            }
        }
    }

    async fn trigger_breaker(&self, reason: &str) {
        if self.circuit_breaker_triggered() {
            return;
        }

        self.set_circuit_breaker();
        warn!(reason, "circuit breaker triggered");
        let _ = self
            .bus
            .publish("control.strategy.pause_all", BusMessage(Vec::new()))
            .await;
        let _ = self
            .bus
            .publish("control.exec.block_new_orders", BusMessage(Vec::new()))
            .await;

        let evt = LogEvent {
            ts: chrono::Utc::now(),
            level: LogLevel::Error,
            source: LogSource::Kernel,
            message: "circuit breaker triggered".to_string(),
            fields: serde_json::json!({"reason": reason}),
            correlation_id: None,
            trace_id: None,
        };
        if let Err(err) = self
            .bus
            .publish("log.event", BusMessage(serde_json::to_vec(&evt).unwrap()))
            .await
        {
            error!(?err, "failed to publish circuit breaker log");
        }
    }

    async fn run(self: Arc<Self>) -> Result<()> {
        let mut order_sub = self.bus.subscribe("orders.result").await?;
        let mut log_sub = self.bus.subscribe("log.event").await?;
        let mut config_sub = self.bus.subscribe(SUBJECT_RISK_UPDATE).await?;
        let mut ticker = interval(Duration::from_secs(5));

        loop {
            tokio::select! {
                Some(msg) = order_sub.receiver.recv() => {
                    if let Ok(result) = serde_json::from_slice::<OrderResult>(&msg.0) {
                        self.record_order(&result);
                        self.evaluate().await;
                    }
                }
                Some(msg) = log_sub.receiver.recv() => {
                    if let Ok(event) = serde_json::from_slice::<LogEvent>(&msg.0) {
                        if matches!(event.level, LogLevel::Error) {
                            self.record_error();
                            self.evaluate().await;
                        }
                    }
                }
                Some(msg) = config_sub.receiver.recv() => {
                    if let Ok(new_cfg) = serde_json::from_slice::<GlobalRiskConfig>(&msg.0) {
                        let mut guard = self.config.write().await;
                        *guard = new_cfg;
                    }
                }
                _ = ticker.tick() => {
                    self.evaluate().await;
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Service for RiskSupervisorService {
    fn id(&self) -> &ServiceId {
        &self.id
    }

    async fn start(&self) -> Result<()> {
        let svc = Arc::new(self.clone());
        let handle = tokio::spawn(async move {
            if let Err(err) = svc.run().await {
                error!(?err, "risk supervisor exited with error");
            }
        });

        let mut guard = self.task_handle.lock().unwrap();
        *guard = Some(handle);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
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
            anyhow::bail!("risk supervisor not running")
        }
    }
}

impl Clone for RiskSupervisorService {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            config: self.config.clone(),
            bus: Arc::clone(&self.bus),
            state: Mutex::new(self.state.lock().unwrap().clone()),
            task_handle: Mutex::new(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::timeout;
    use w2uos_bus::LocalBus;
    use w2uos_data::{ExchangeId, Symbol};

    #[tokio::test]
    async fn triggers_circuit_on_loss() {
        let bus: Arc<dyn MessageBus> = Arc::new(LocalBus::new());
        let config = GlobalRiskConfig {
            max_daily_loss_pct: 5.0,
            max_daily_loss_abs: 100.0,
            max_total_notional: 1_000_000.0,
            max_open_positions: 10,
            max_error_rate_per_minute: None,
        };
        let svc = Arc::new(RiskSupervisorService::new(
            "risk".into(),
            config,
            Arc::clone(&bus),
        ));

        let mut ctrl_sub = bus
            .subscribe("control.exec.block_new_orders")
            .await
            .unwrap();
        svc.start().await.unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        let loss_order = OrderResult {
            command_id: "1".into(),
            exchange: ExchangeId::Okx,
            symbol: Symbol {
                base: "BTC".into(),
                quote: "USDT".into(),
            },
            side: OrderSide::Buy,
            status: w2uos_exec::OrderStatus::Filled,
            filled_size_quote: 200.0,
            avg_price: 20_000.0,
            ts: chrono::Utc::now(),
            trace_id: None,
        };

        let payload = serde_json::to_vec(&loss_order).unwrap();
        bus.publish("orders.result", BusMessage(payload))
            .await
            .unwrap();

        timeout(Duration::from_secs(2), ctrl_sub.receiver.recv())
            .await
            .expect("breaker not triggered")
            .expect("control message missing");
    }
}
