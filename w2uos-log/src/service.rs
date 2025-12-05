use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use w2uos_bus::{BusMessage, MessageBus};
use w2uos_service::{Service, ServiceId};

use std::collections::HashMap;

use crate::sink::LogSink;
use crate::types::{LatencyBucket, LatencyRecord, LatencySummary, LogEvent, TradeRecord};

pub struct LogService {
    pub sink: Arc<dyn LogSink>,
    pub bus: Arc<dyn MessageBus>,
    shutdown: watch::Sender<bool>,
    recent_events: Arc<Mutex<Vec<LogEvent>>>,
    latency_state: Arc<Mutex<HashMap<String, PartialLatencyState>>>,
    latency_records: Arc<Mutex<Vec<LatencyRecord>>>,
}

impl LogService {
    pub fn new(sink: Arc<dyn LogSink>, bus: Arc<dyn MessageBus>) -> Self {
        let (shutdown, _) = watch::channel(false);
        Self {
            sink,
            bus,
            shutdown,
            recent_events: Arc::new(Mutex::new(Vec::new())),
            latency_state: Arc::new(Mutex::new(HashMap::new())),
            latency_records: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn record_latency(
        &self,
        trace_id: String,
        stage: &str,
        ts: chrono::DateTime<chrono::Utc>,
    ) {
        let mut guard = self.latency_state.lock().await;
        let entry = guard
            .entry(trace_id.clone())
            .or_insert_with(PartialLatencyState::default);

        match stage {
            "tick" => entry.ts_start_tick = Some(ts),
            "strategy_decision" => entry.ts_strategy_decision = Some(ts),
            "order_result" => entry.ts_order_result = Some(ts),
            _ => {}
        }

        if let (Some(start), Some(decision), Some(result)) = (
            entry.ts_start_tick,
            entry.ts_strategy_decision,
            entry.ts_order_result,
        ) {
            let latency = LatencyRecord {
                trace_id: w2uos_service::TraceId(trace_id.clone()),
                ts_start_tick: start,
                ts_strategy_decision: decision,
                ts_order_result: result,
                tick_to_strategy_ms: (decision - start).num_milliseconds(),
                strategy_to_exec_ms: (result - decision).num_milliseconds(),
                tick_to_result_ms: (result - start).num_milliseconds(),
            };

            guard.remove(&trace_id);
            {
                let mut records = self.latency_records.lock().await;
                records.push(latency.clone());
                if records.len() > 2000 {
                    let excess = records.len() - 2000;
                    records.drain(0..excess);
                }
            }

            if let Err(err) = self.sink.write_latency(latency).await {
                error!(?err, "failed to persist latency record");
            }
        }
    }

    async fn handle_event(&self, bytes: &[u8]) {
        match serde_json::from_slice::<LogEvent>(bytes) {
            Ok(event) => {
                {
                    let mut guard = self.recent_events.lock().await;
                    guard.push(event.clone());
                    if guard.len() > 1000 {
                        let excess = guard.len() - 1000;
                        guard.drain(0..excess);
                    }
                }
                if let (Some(trace_id), Some(stage)) = (
                    event.trace_id.as_ref(),
                    event.fields.get("stage").and_then(|v| v.as_str()),
                ) {
                    self.record_latency(trace_id.0.clone(), stage, event.ts)
                        .await;
                }
                if let Err(err) = self.sink.write_event(event).await {
                    error!(?err, "failed to persist log event");
                }
            }
            Err(err) => warn!(?err, "failed to decode log event"),
        }
    }

    async fn handle_trade(&self, bytes: &[u8]) {
        match serde_json::from_slice::<TradeRecord>(bytes) {
            Ok(trade) => {
                if let Err(err) = self.sink.write_trade(trade).await {
                    error!(?err, "failed to persist trade record");
                }
            }
            Err(err) => warn!(?err, "failed to decode trade record"),
        }
    }

    pub async fn log(&self, event: LogEvent) -> Result<()> {
        self.sink.write_event(event).await
    }

    pub async fn log_trade(&self, trade: TradeRecord) -> Result<()> {
        self.sink.write_trade(trade).await
    }

    pub async fn run(self: Arc<Self>) -> Result<()> {
        info!("log service running");
        let mut event_sub = self.bus.subscribe("log.event").await?;
        let mut trade_sub = self.bus.subscribe("log.trade").await?;
        let mut shutdown_rx = self.shutdown.subscribe();

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
                Some(msg) = event_sub.receiver.recv() => {
                    self.handle_event(&msg.0).await;
                }
                Some(msg) = trade_sub.receiver.recv() => {
                    self.handle_trade(&msg.0).await;
                }
                else => break,
            }
        }

        Ok(())
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    pub async fn recent_events(&self, limit: usize) -> Vec<LogEvent> {
        let guard = self.recent_events.lock().await;
        let len = guard.len();
        let start = len.saturating_sub(limit);
        guard[start..].to_vec()
    }

    pub async fn latency_summary(&self) -> LatencySummary {
        let guard = self.latency_records.lock().await;
        if guard.is_empty() {
            return LatencySummary::default();
        }
        let mut values: Vec<i64> = guard.iter().map(|r| r.tick_to_result_ms).collect();
        values.sort_unstable();
        let min = *values.first().unwrap();
        let sum: i64 = values.iter().sum();
        let avg = sum as f64 / values.len() as f64;
        let p95_idx = ((values.len() as f64) * 0.95).ceil() as usize - 1;
        let p99_idx = ((values.len() as f64) * 0.99).ceil() as usize - 1;

        LatencySummary {
            min_ms: Some(min as f64),
            avg_ms: Some(avg),
            p95_ms: Some(*values.get(p95_idx).unwrap_or(values.last().unwrap()) as f64),
            p99_ms: Some(*values.get(p99_idx).unwrap_or(values.last().unwrap()) as f64),
        }
    }

    pub async fn latency_histogram(&self) -> Vec<LatencyBucket> {
        let guard = self.latency_records.lock().await;
        let mut buckets = vec![
            LatencyBucket {
                upper_bound_ms: 10,
                count: 0,
            },
            LatencyBucket {
                upper_bound_ms: 25,
                count: 0,
            },
            LatencyBucket {
                upper_bound_ms: 50,
                count: 0,
            },
            LatencyBucket {
                upper_bound_ms: 100,
                count: 0,
            },
            LatencyBucket {
                upper_bound_ms: 250,
                count: 0,
            },
            LatencyBucket {
                upper_bound_ms: 500,
                count: 0,
            },
            LatencyBucket {
                upper_bound_ms: 1_000,
                count: 0,
            },
            LatencyBucket {
                upper_bound_ms: 5_000,
                count: 0,
            },
        ];

        for value in guard.iter().map(|r| r.tick_to_result_ms) {
            if let Some(bucket) = buckets.iter_mut().find(|b| value <= b.upper_bound_ms) {
                bucket.count += 1;
            }
        }

        buckets
    }
}

#[derive(Default, Clone, Copy)]
struct PartialLatencyState {
    ts_start_tick: Option<chrono::DateTime<chrono::Utc>>,
    ts_strategy_decision: Option<chrono::DateTime<chrono::Utc>>,
    ts_order_result: Option<chrono::DateTime<chrono::Utc>>,
}

pub struct LogKernelService {
    id: ServiceId,
    svc: Arc<LogService>,
    task_handle: Mutex<Option<JoinHandle<()>>>,
}

impl LogKernelService {
    pub fn new(id: ServiceId, svc: Arc<LogService>) -> Self {
        Self {
            id,
            svc,
            task_handle: Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl Service for LogKernelService {
    fn id(&self) -> &ServiceId {
        &self.id
    }

    async fn start(&self) -> Result<()> {
        let svc = Arc::clone(&self.svc);
        let handle = tokio::spawn(async move {
            if let Err(err) = svc.run().await {
                error!(?err, "log service terminated with error");
            }
        });

        let mut guard = self.task_handle.lock().await;
        *guard = Some(handle);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!(service = %self.id, "log service shutdown requested");
        self.svc.shutdown();
        Ok(())
    }

    async fn health_check(&self) -> Result<()> {
        let guard = self.task_handle.lock().await;
        if let Some(handle) = guard.as_ref() {
            if handle.is_finished() {
                anyhow::bail!("log service task finished unexpectedly");
            }
            Ok(())
        } else {
            anyhow::bail!("log service not started")
        }
    }
}

pub async fn log_event_via_bus(bus: &dyn MessageBus, event: &LogEvent) -> Result<()> {
    let payload = serde_json::to_vec(event)?;
    bus.publish("log.event", BusMessage(payload)).await
}

pub async fn log_trade_via_bus(bus: &dyn MessageBus, trade: &TradeRecord) -> Result<()> {
    let payload = serde_json::to_vec(trade)?;
    bus.publish("log.trade", BusMessage(payload)).await
}
