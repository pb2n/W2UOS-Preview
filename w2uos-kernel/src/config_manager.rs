use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration};
use tracing::{info, warn};
use w2uos_bus::{BusMessage, MessageBus};
use w2uos_config::{LoggingConfig, NodeConfig};
use w2uos_service::{Service, ServiceId};

pub const SUBJECT_RISK_UPDATE: &str = "control.config.update.risk";
pub const SUBJECT_EXEC_UPDATE: &str = "control.config.update.execution";
pub const SUBJECT_STRATEGY_UPDATE: &str = "control.config.update.strategy";
pub const SUBJECT_MARKET_UPDATE: &str = "control.config.update.market";
pub const SUBJECT_LOGGING_UPDATE: &str = "control.config.update.logging";

#[derive(Clone)]
pub struct ConfigManagerService {
    id: ServiceId,
    config_path: PathBuf,
    env_name: Option<String>,
    bus: Arc<dyn MessageBus>,
    current: Arc<RwLock<NodeConfig>>,
    worker: Arc<RwLock<Option<JoinHandle<()>>>>,
    last_modified: Arc<RwLock<Option<SystemTime>>>,
}

impl ConfigManagerService {
    pub fn new(
        id: ServiceId,
        config_path: PathBuf,
        env_name: Option<String>,
        bus: Arc<dyn MessageBus>,
        seed: NodeConfig,
    ) -> Self {
        Self {
            id,
            config_path,
            env_name,
            bus,
            current: Arc::new(RwLock::new(seed)),
            worker: Arc::new(RwLock::new(None)),
            last_modified: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn current(&self) -> NodeConfig {
        self.current.read().await.clone()
    }

    pub async fn reload_now(&self) -> Result<()> {
        let cfg = NodeConfig::load_with_env(&self.config_path, self.env_name.clone())?;
        {
            let mut guard = self.current.write().await;
            *guard = cfg.clone();
        }
        apply_logging_config(&cfg.logging)?;
        self.publish_updates(&cfg).await?;
        Ok(())
    }

    async fn publish_updates(&self, cfg: &NodeConfig) -> Result<()> {
        let risk_payload = serde_json::to_vec(&cfg.global_risk)?;
        self.bus
            .publish(SUBJECT_RISK_UPDATE, BusMessage(risk_payload))
            .await?;

        let exec_payload = serde_json::to_vec(&cfg.execution)?;
        self.bus
            .publish(SUBJECT_EXEC_UPDATE, BusMessage(exec_payload))
            .await?;

        let strat_payload = serde_json::to_vec(&cfg.strategy.bot_config)?;
        self.bus
            .publish(SUBJECT_STRATEGY_UPDATE, BusMessage(strat_payload))
            .await?;

        let md_payload = serde_json::to_vec(&cfg.market_data)?;
        self.bus
            .publish(SUBJECT_MARKET_UPDATE, BusMessage(md_payload))
            .await?;

        let logging_payload = serde_json::to_vec(&cfg.logging)?;
        self.bus
            .publish(SUBJECT_LOGGING_UPDATE, BusMessage(logging_payload))
            .await?;

        Ok(())
    }

    async fn watch(self: Arc<Self>) {
        let mut ticker = interval(Duration::from_secs(5));
        loop {
            ticker.tick().await;
            match std::fs::metadata(&self.config_path) {
                Ok(meta) => {
                    let modified = meta.modified().ok();
                    let mut guard = self.last_modified.write().await;
                    if guard.is_none() {
                        *guard = modified;
                        if let Err(err) = self.reload_now().await {
                            warn!(?err, "initial config reload failed");
                        }
                        continue;
                    }

                    if modified.is_some() && modified != *guard {
                        *guard = modified;
                        info!("config change detected, reloading");
                        if let Err(err) = self.reload_now().await {
                            warn!(?err, "failed to reload config");
                        }
                    }
                }
                Err(err) => warn!(?err, "failed to stat config file"),
            }
        }
    }
}

#[async_trait::async_trait]
impl Service for ConfigManagerService {
    fn id(&self) -> &ServiceId {
        &self.id
    }

    async fn start(&self) -> Result<()> {
        let worker = tokio::spawn(Arc::new(self.clone()).watch());
        let mut guard = self.worker.write().await;
        *guard = Some(worker);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        if let Some(handle) = self.worker.write().await.take() {
            handle.abort();
        }
        Ok(())
    }

    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

pub fn apply_logging_config(cfg: &LoggingConfig) -> Result<()> {
    crate::Kernel::reload_tracing_filter(&cfg.level)
}
