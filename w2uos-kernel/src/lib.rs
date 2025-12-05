use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Result;
use tracing::info;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, reload, EnvFilter, Registry};
use w2uos_bus::MessageBus;

pub use w2uos_service::{Service, ServiceId};

pub mod cluster;
pub use cluster::{ClusterService, NodeHeartbeat, NodeId, NodeRole};
pub mod strategy_host;
pub use strategy_host::{
    StrategyBackend, StrategyHostConfig, StrategyHostService, StrategySubscription,
};
pub mod risk_config;
pub mod risk_supervisor;
pub use risk_config::GlobalRiskConfig;
pub use risk_supervisor::{RiskStatus, RiskSupervisorService};
pub mod config_manager;
pub use config_manager::ConfigManagerService;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelState {
    Initialized,
    Running,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelMode {
    Live,
    Backtest,
}

impl Default for KernelMode {
    fn default() -> Self {
        KernelMode::Live
    }
}

impl Default for KernelState {
    fn default() -> Self {
        KernelState::Initialized
    }
}

/// Core runtime and actor orchestration for W2UOS.
#[derive(Default)]
pub struct Kernel {
    bus: Option<Arc<dyn MessageBus>>, // placeholder until wiring is complete
    services: Vec<Arc<dyn Service>>,  // simple service registry
    state: Arc<Mutex<KernelState>>,   // lightweight lifecycle tracker
    service_states: Arc<Mutex<HashMap<ServiceId, String>>>,
    mode: KernelMode,
}

static TRACING_HANDLE: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();

impl Kernel {
    /// Initialize tracing for the runtime with a verbose, env-configurable format.
    pub fn init_tracing() {
        let env_filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let fmt_layer = fmt::layer()
            .with_target(true)
            .with_thread_names(true)
            .with_file(true)
            .with_line_number(true)
            .compact();
        let (filter_layer, handle) = reload::Layer::new(env_filter);
        let subscriber = tracing_subscriber::registry()
            .with(filter_layer)
            .with(fmt_layer);
        let _ = tracing::subscriber::set_global_default(subscriber);
        let _ = TRACING_HANDLE.set(handle);
    }

    pub fn reload_tracing_filter(level: &str) -> anyhow::Result<()> {
        let filter = EnvFilter::try_new(level)?;
        if let Some(handle) = TRACING_HANDLE.get() {
            handle.reload(filter)?;
        }
        Ok(())
    }

    /// Start the kernel runtime.
    pub async fn start(&self) -> Result<()> {
        info!("W2UOS kernel started");
        Ok(())
    }

    /// Start all registered services.
    pub async fn start_all(&self) -> Result<()> {
        self.start().await?;
        for svc in &self.services {
            info!(service = %svc.id(), "starting service");
            svc.start().await?;
            let mut guard = self.service_states.lock().unwrap();
            guard.insert(svc.id().clone(), "Running".to_string());
        }
        let mut state_guard = self.state.lock().unwrap();
        *state_guard = KernelState::Running;
        Ok(())
    }

    /// Attach a message bus implementation to the kernel.
    pub fn with_message_bus(mut self, bus: Arc<dyn MessageBus>) -> Self {
        self.bus = Some(bus);
        self
    }

    pub fn with_mode(mut self, mode: KernelMode) -> Self {
        self.mode = mode;
        self
    }

    /// Register a service with the kernel for lifecycle management.
    pub fn register_service(&mut self, svc: Arc<dyn Service>) {
        self.services.push(svc);
        if let Some(last) = self.services.last() {
            let mut guard = self.service_states.lock().unwrap();
            guard.insert(last.id().clone(), "Registered".to_string());
        }
    }

    /// Snapshot current kernel lifecycle state.
    pub async fn state(&self) -> KernelState {
        self.state.lock().unwrap().clone()
    }

    pub fn mode(&self) -> KernelMode {
        self.mode
    }

    /// Snapshot of known service states.
    pub async fn service_states(&self) -> HashMap<ServiceId, String> {
        self.service_states.lock().unwrap().clone()
    }
}
