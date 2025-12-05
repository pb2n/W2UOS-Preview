use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{mpsc, Mutex, RwLock},
    task::JoinHandle,
    time::interval,
};
use tracing::{error, info, warn};
use w2uos_bus::{BusMessage, MessageBus};
use w2uos_log::{log_event_via_bus, LogEvent, LogLevel, LogSource};
use w2uos_net::{build_client_and_shaper, NetMode, NetProfile, TrafficShaper};
use w2uos_service::{Service, ServiceId, TraceId};

use crate::{
    history::HistoricalStore,
    okx::OkxMarketStream,
    types::{ExchangeId, MarketSnapshot, Symbol},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DataMode {
    Simulated,
    LiveOkx,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MarketDataSubscription {
    pub exchange: ExchangeId,
    pub symbol: Symbol,
    pub ws_url: String,
}

impl Default for DataMode {
    fn default() -> Self {
        DataMode::Simulated
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MarketDataConfig {
    pub subscriptions: Vec<MarketDataSubscription>,
    pub net_profile: NetProfile,
    pub history: Option<MarketHistoryConfig>,
    #[serde(default)]
    pub mode: DataMode,
}

impl Default for MarketDataConfig {
    fn default() -> Self {
        Self {
            subscriptions: vec![],
            net_profile: NetProfile::default(),
            history: None,
            mode: DataMode::default(),
        }
    }
}

pub struct MarketDataService {
    pub bus: Arc<dyn MessageBus>,
    pub config: Arc<RwLock<MarketDataConfig>>,
    pub http_client: Arc<RwLock<Client>>,
    pub traffic_shaper: Arc<RwLock<TrafficShaper>>,
    history_tx: Option<mpsc::Sender<MarketSnapshot>>,
    #[allow(dead_code)]
    history_task: Option<JoinHandle<()>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MarketHistoryConfig {
    pub connection_string: String,
}

impl MarketDataService {
    pub async fn new(bus: Arc<dyn MessageBus>, config: MarketDataConfig) -> Result<Self> {
        let (http_client, traffic_shaper) = build_client_and_shaper(&config.net_profile)?;
        let (history_tx, history_task) = if let Some(history_cfg) = &config.history {
            let store = HistoricalStore::connect(&history_cfg.connection_string).await?;
            let (tx, mut rx) = mpsc::channel(1024);
            let handle = tokio::spawn(async move {
                while let Some(snapshot) = rx.recv().await {
                    if let Err(err) = store.insert_snapshot(&snapshot).await {
                        error!(?err, "failed to persist snapshot to history store");
                    }
                }
            });
            (Some(tx), Some(handle))
        } else {
            (None, None)
        };

        Ok(Self {
            bus,
            http_client: Arc::new(RwLock::new(http_client)),
            traffic_shaper: Arc::new(RwLock::new(traffic_shaper)),
            config: Arc::new(RwLock::new(config)),
            history_tx,
            history_task,
        })
    }

    pub async fn run(self: Arc<Self>) -> Result<()> {
        info!("market data service running");
        let cfg_snapshot = self.config.read().await.clone();
        if matches!(
            cfg_snapshot.net_profile.mode,
            NetMode::Socks5Proxy | NetMode::Tor
        ) {
            info!(mode = %cfg_snapshot.net_profile.mode, "market data service using proxied network mode");
        }

        let cfg_handle = Arc::clone(&self.config);
        let client_handle = Arc::clone(&self.http_client);
        let shaper_handle = Arc::clone(&self.traffic_shaper);
        let bus_for_config = Arc::clone(&self.bus);
        tokio::spawn(async move {
            if let Ok(mut sub) = bus_for_config
                .subscribe("control.config.update.market")
                .await
            {
                while let Some(msg) = sub.receiver.recv().await {
                    if let Ok(new_cfg) = serde_json::from_slice::<MarketDataConfig>(&msg.0) {
                        if let Ok((new_client, new_shaper)) =
                            build_client_and_shaper(&new_cfg.net_profile)
                        {
                            let mut client_guard = client_handle.write().await;
                            *client_guard = new_client;
                            let mut shaper_guard = shaper_handle.write().await;
                            *shaper_guard = new_shaper;
                        }
                        let mut guard = cfg_handle.write().await;
                        *guard = new_cfg;
                    }
                }
            }
        });
        let start_event = LogEvent {
            ts: chrono::Utc::now(),
            level: LogLevel::Info,
            source: LogSource::MarketData,
            message: "market data service started".to_string(),
            fields: serde_json::json!({"subscriptions": cfg_snapshot.subscriptions.len()}),
            correlation_id: None,
            trace_id: None,
        };

        if let Err(err) = log_event_via_bus(self.bus.as_ref(), &start_event).await {
            error!(?err, "failed to publish market data start event");
        }

        match cfg_snapshot.mode {
            DataMode::LiveOkx => self.run_okx().await?,
            DataMode::Simulated => self.run_simulated().await?,
        }

        Ok(())
    }

    async fn run_okx(&self) -> Result<()> {
        let cfg = self.config.read().await.clone();
        if cfg.subscriptions.is_empty() {
            anyhow::bail!("no subscriptions configured for OKX mode");
        }
        let ws_url = cfg
            .subscriptions
            .first()
            .map(|s| s.ws_url.clone())
            .ok_or_else(|| anyhow::anyhow!("missing OKX websocket url"))?;

        let stream = OkxMarketStream {
            bus: Arc::clone(&self.bus),
            ws_url,
            subscriptions: cfg.subscriptions.clone(),
            history_tx: self.history_tx.clone(),
        };

        stream.run().await
    }

    async fn run_simulated(&self) -> Result<()> {
        let mut price_state: HashMap<String, f64> = HashMap::new();
        let mut ticker = interval(Duration::from_millis(500));

        loop {
            ticker.tick().await;
            let cfg = self.config.read().await.clone();
            for sub in &cfg.subscriptions {
                let key = format!("{}:{}{}", sub.exchange, sub.symbol.base, sub.symbol.quote);
                let price = price_state
                    .entry(key)
                    .and_modify(|p| *p += 5.0)
                    .or_insert(30_000.0);
                let trace_id = Some(TraceId::new());
                let snapshot = MarketSnapshot {
                    ts: chrono::Utc::now(),
                    exchange: sub.exchange.clone(),
                    symbol: sub.symbol.clone(),
                    last: *price,
                    bid: *price - 1.5,
                    ask: *price + 1.5,
                    volume_24h: (*price / 1000.0) * 10.0,
                    trace_id: trace_id.clone(),
                };

                let subject = format!(
                    "market.{}.{}{}",
                    snapshot.exchange,
                    snapshot.symbol.base.to_uppercase(),
                    snapshot.symbol.quote.to_uppercase()
                );

                match serde_json::to_vec(&snapshot) {
                    Ok(payload) => {
                        if let Err(err) = self.bus.publish(&subject, BusMessage(payload)).await {
                            let evt = LogEvent {
                                ts: chrono::Utc::now(),
                                level: LogLevel::Error,
                                source: LogSource::MarketData,
                                message: "failed to publish snapshot".to_string(),
                                fields: serde_json::json!({"subject": subject, "error": err.to_string()}),
                                correlation_id: None,
                                trace_id: snapshot.trace_id.clone(),
                            };
                            let _ = log_event_via_bus(self.bus.as_ref(), &evt).await;
                            error!(%subject, ?err, "failed to publish snapshot");
                        }
                    }
                    Err(err) => {
                        let evt = LogEvent {
                            ts: chrono::Utc::now(),
                            level: LogLevel::Error,
                            source: LogSource::MarketData,
                            message: "failed to serialize snapshot".to_string(),
                            fields: serde_json::json!({"subject": subject, "error": err.to_string()}),
                            correlation_id: None,
                            trace_id: snapshot.trace_id.clone(),
                        };
                        let _ = log_event_via_bus(self.bus.as_ref(), &evt).await;
                        error!(?err, "failed to serialize snapshot");
                    }
                }

                if let Some(tx) = self.history_tx.as_ref() {
                    if let Err(err) = tx.try_send(snapshot.clone()) {
                        warn!(?err, "history channel full, dropping snapshot");
                    }
                }

                if let Some(trace_id) = trace_id.clone() {
                    let evt = LogEvent {
                        ts: snapshot.ts,
                        level: LogLevel::Info,
                        source: LogSource::MarketData,
                        message: "market tick".to_string(),
                        fields: serde_json::json!({"stage": "tick", "subject": subject }),
                        correlation_id: None,
                        trace_id: Some(trace_id),
                    };
                    if let Err(err) = log_event_via_bus(self.bus.as_ref(), &evt).await {
                        error!(?err, "failed to publish tick event");
                    }
                }
            }
        }
    }
}

pub struct MarketDataKernelService {
    id: ServiceId,
    svc: Arc<MarketDataService>,
    task_handle: Mutex<Option<JoinHandle<()>>>,
}

impl MarketDataKernelService {
    pub fn new(id: ServiceId, svc: Arc<MarketDataService>) -> Self {
        Self {
            id,
            svc,
            task_handle: Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl Service for MarketDataKernelService {
    fn id(&self) -> &ServiceId {
        &self.id
    }

    async fn start(&self) -> Result<()> {
        let svc = Arc::clone(&self.svc);
        let handle = tokio::spawn(async move {
            if let Err(err) = svc.run().await {
                error!(?err, "market data service terminated with error");
            }
        });

        let mut guard = self.task_handle.lock().await;
        *guard = Some(handle);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!(service = %self.id, "stop requested for market data service (not yet implemented)");
        Ok(())
    }

    async fn health_check(&self) -> Result<()> {
        let guard = self.task_handle.lock().await;
        if guard.is_some() {
            Ok(())
        } else {
            anyhow::bail!("market data service not started")
        }
    }
}
