use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradingState {
    Booting,
    Simulating,
    LiveDryRun,
    LiveArmed,
    Paused,
    Frozen,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskProfile {
    UltraConservative,
    Conservative,
    Default,
    Aggressive,
}

impl Default for RiskProfile {
    fn default() -> Self {
        RiskProfile::Default
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RiskLimits {
    pub max_leverage: i32,
    pub max_notional_usdt: f64,
    pub max_concurrent_positions: i32,
}

impl Default for RiskLimits {
    fn default() -> Self {
        RiskProfile::Default.limits()
    }
}

#[derive(Clone, Debug, Default)]
pub struct ControlPlaneState {
    pub trading_state: TradingState,
    pub paused_from: Option<TradingState>,
    pub frozen_from: Option<TradingState>,
    pub freeze_cancel_open_orders: bool,
    pub flatten_request: Option<FlattenRequest>,
    pub risk_profile: RiskProfile,
    pub risk_limits: RiskLimits,
    pub live_switch_armed: bool,
    pub live_switch_reason: Option<String>,
    pub live_switch_updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Debug)]
pub struct FlattenRequest {
    pub reason: Option<String>,
    pub mode: Option<String>,
    pub timeout_ms: Option<u64>,
    pub requested_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug)]
pub struct TradingStateTransition {
    pub previous: TradingState,
    pub new_state: TradingState,
}

#[derive(Clone, Debug)]
pub struct RiskProfileTransition {
    pub previous: RiskProfile,
    pub new_profile: RiskProfile,
    pub limits: RiskLimits,
}

impl TradingState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TradingState::Booting => "BOOTING",
            TradingState::Simulating => "SIMULATING",
            TradingState::LiveDryRun => "LIVE_DRY_RUN",
            TradingState::LiveArmed => "LIVE_ARMED",
            TradingState::Paused => "PAUSED",
            TradingState::Frozen => "FROZEN",
            TradingState::Error => "ERROR",
        }
    }
}

impl Default for TradingState {
    fn default() -> Self {
        TradingState::Booting
    }
}

impl RiskProfile {
    pub fn as_str(&self) -> &'static str {
        match self {
            RiskProfile::UltraConservative => "ULTRA_CONSERVATIVE",
            RiskProfile::Conservative => "CONSERVATIVE",
            RiskProfile::Default => "DEFAULT",
            RiskProfile::Aggressive => "AGGRESSIVE",
        }
    }

    pub fn limits(&self) -> RiskLimits {
        match self {
            RiskProfile::UltraConservative => RiskLimits {
                max_leverage: 3,
                max_notional_usdt: 50.0,
                max_concurrent_positions: 1,
            },
            RiskProfile::Conservative => RiskLimits {
                max_leverage: 5,
                max_notional_usdt: 100.0,
                max_concurrent_positions: 2,
            },
            RiskProfile::Default => RiskLimits {
                max_leverage: 10,
                max_notional_usdt: 200.0,
                max_concurrent_positions: 4,
            },
            RiskProfile::Aggressive => RiskLimits {
                max_leverage: 20,
                max_notional_usdt: 500.0,
                max_concurrent_positions: 8,
            },
        }
    }
}

/// Core runtime and actor orchestration for W2UOS.
pub struct Kernel {
    bus: Option<Arc<dyn MessageBus>>, // placeholder until wiring is complete
    services: Vec<Arc<dyn Service>>,  // simple service registry
    state: Arc<Mutex<KernelState>>,   // lightweight lifecycle tracker
    service_states: Arc<Mutex<HashMap<ServiceId, String>>>,
    mode: KernelMode,
    control_state: Arc<Mutex<ControlPlaneState>>,
}

impl Default for Kernel {
    fn default() -> Self {
        Self {
            bus: None,
            services: Vec::new(),
            state: Arc::new(Mutex::new(KernelState::default())),
            service_states: Arc::new(Mutex::new(HashMap::new())),
            mode: KernelMode::default(),
            control_state: Arc::new(Mutex::new(ControlPlaneState {
                trading_state: TradingState::default(),
                risk_profile: RiskProfile::default(),
                risk_limits: RiskProfile::Default.limits(),
                ..Default::default()
            })),
        }
    }
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

    pub async fn trading_state(&self) -> TradingState {
        self.control_state.lock().unwrap().trading_state
    }

    pub fn set_trading_state(&self, state: TradingState) {
        let mut guard = self.control_state.lock().unwrap();
        guard.trading_state = state;
    }

    pub fn switch_trading_state(&self, target: TradingState) -> Result<TradingStateTransition> {
        let mut guard = self.control_state.lock().unwrap();
        if guard.trading_state == TradingState::Frozen && target == TradingState::LiveArmed {
            anyhow::bail!("cannot arm trading while frozen");
        }
        if guard.trading_state == TradingState::Error && target == TradingState::LiveArmed {
            anyhow::bail!("cannot arm trading while in error state");
        }

        let previous = guard.trading_state;
        guard.trading_state = target;
        if target != TradingState::Paused {
            guard.paused_from = None;
        }
        if target != TradingState::Frozen {
            guard.frozen_from = None;
            guard.freeze_cancel_open_orders = false;
        }

        Ok(TradingStateTransition {
            previous,
            new_state: target,
        })
    }

    pub fn pause(&self) -> TradingStateTransition {
        let mut guard = self.control_state.lock().unwrap();
        let previous = guard.trading_state;
        if guard.trading_state != TradingState::Paused {
            guard.paused_from = Some(guard.trading_state);
            guard.trading_state = TradingState::Paused;
        }

        TradingStateTransition {
            previous,
            new_state: TradingState::Paused,
        }
    }

    pub fn resume(&self) -> Result<TradingStateTransition> {
        let mut guard = self.control_state.lock().unwrap();
        if guard.trading_state != TradingState::Paused {
            anyhow::bail!("resume allowed only from PAUSED state")
        }

        let target = guard.paused_from.unwrap_or(TradingState::LiveDryRun);
        let previous = guard.trading_state;
        guard.trading_state = target;
        guard.paused_from = None;

        Ok(TradingStateTransition {
            previous,
            new_state: target,
        })
    }

    pub fn freeze(&self, cancel_open_orders: bool) -> TradingStateTransition {
        let mut guard = self.control_state.lock().unwrap();
        let previous = guard.trading_state;
        guard.frozen_from = Some(guard.trading_state);
        guard.trading_state = TradingState::Frozen;
        guard.freeze_cancel_open_orders = cancel_open_orders;

        TradingStateTransition {
            previous,
            new_state: TradingState::Frozen,
        }
    }

    pub fn unfreeze(&self, target: TradingState) -> Result<TradingStateTransition> {
        let mut guard = self.control_state.lock().unwrap();
        if guard.trading_state != TradingState::Frozen {
            anyhow::bail!("unfreeze allowed only from FROZEN state")
        }

        let previous = guard.trading_state;
        guard.trading_state = target;
        guard.frozen_from = None;
        guard.freeze_cancel_open_orders = false;

        Ok(TradingStateTransition {
            previous,
            new_state: target,
        })
    }

    pub fn request_flatten(
        &self,
        reason: Option<String>,
        mode: Option<String>,
        timeout_ms: Option<u64>,
    ) {
        let mut guard = self.control_state.lock().unwrap();
        guard.flatten_request = Some(FlattenRequest {
            reason,
            mode,
            timeout_ms,
            requested_at: Utc::now(),
        });
    }

    pub fn set_risk_profile(&self, profile: RiskProfile) -> RiskProfileTransition {
        let mut guard = self.control_state.lock().unwrap();
        let previous = guard.risk_profile;
        let limits = profile.limits();
        guard.risk_profile = profile;
        guard.risk_limits = limits;

        RiskProfileTransition {
            previous,
            new_profile: profile,
            limits,
        }
    }

    pub async fn risk_profile(&self) -> (RiskProfile, RiskLimits) {
        let guard = self.control_state.lock().unwrap();
        (guard.risk_profile, guard.risk_limits)
    }

    pub async fn control_snapshot(&self) -> ControlPlaneState {
        self.control_state.lock().unwrap().clone()
    }

    pub fn set_live_switch(&self, armed: bool, reason: Option<String>) -> bool {
        let mut guard = self.control_state.lock().unwrap();
        guard.live_switch_armed = armed;
        guard.live_switch_reason = reason;
        guard.live_switch_updated_at = Some(Utc::now());
        armed
    }

    pub async fn live_switch_state(
        &self,
    ) -> (bool, Option<String>, Option<chrono::DateTime<chrono::Utc>>) {
        let guard = self.control_state.lock().unwrap();
        (
            guard.live_switch_armed,
            guard.live_switch_reason.clone(),
            guard.live_switch_updated_at,
        )
    }

    /// Snapshot of known service states.
    pub async fn service_states(&self) -> HashMap<ServiceId, String> {
        self.service_states.lock().unwrap().clone()
    }
}
