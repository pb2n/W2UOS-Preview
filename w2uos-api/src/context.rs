use std::sync::Arc;

use anyhow::Result;
use w2uos_backtest::{BacktestConfig, BacktestCoordinator};
use w2uos_bus::MessageBus;
use w2uos_data::{ExchangeId, Symbol};
use w2uos_exec::ExecutionService;
use w2uos_kernel::{ClusterService, Kernel, KernelState, RiskSupervisorService};
use w2uos_log::{types::LogEvent, LogService};
use w2uos_net::NetProfile;

use crate::types::{
    ClusterNodeDto, LatencyBucketDto, LatencySummaryDto, LogDto, NodeStatusDto, PositionDto,
    ServiceStatusDto,
};

#[derive(Clone)]
pub struct ApiContext {
    pub kernel: Arc<Kernel>,
    pub bus: Arc<dyn MessageBus>,
    pub log_service: Option<Arc<LogService>>,
    pub exec_service: Option<Arc<ExecutionService>>,
    pub cluster_service: Option<Arc<ClusterService>>,
    pub backtest: Option<Arc<BacktestCoordinator>>,
    pub market_subjects: Vec<String>,
    pub net_profile: NetProfile,
    pub risk_service: Option<Arc<RiskSupervisorService>>,
}

impl ApiContext {
    pub fn new(
        kernel: Arc<Kernel>,
        bus: Arc<dyn MessageBus>,
        log_service: Option<Arc<LogService>>,
        exec_service: Option<Arc<ExecutionService>>,
        cluster_service: Option<Arc<ClusterService>>,
        backtest: Option<Arc<BacktestCoordinator>>,
        market_subjects: Vec<String>,
        net_profile: NetProfile,
        risk_service: Option<Arc<RiskSupervisorService>>,
    ) -> Self {
        Self {
            kernel,
            bus,
            log_service,
            exec_service,
            cluster_service,
            backtest,
            market_subjects,
            net_profile,
            risk_service,
        }
    }

    pub async fn get_node_status(&self) -> Result<NodeStatusDto> {
        let state = self.kernel.state().await;
        let services = self.kernel.service_states().await;
        let services = services
            .into_iter()
            .map(|(id, state)| ServiceStatusDto { id, state })
            .collect();

        Ok(NodeStatusDto {
            kernel_state: format!("{:?}", state),
            kernel_mode: format!("{:?}", self.kernel.mode()),
            services,
        })
    }

    pub async fn health_check(&self) -> Result<bool> {
        let state = self.kernel.state().await;
        let services = self.kernel.service_states().await;
        let services_running = services.values().all(|s| s == "Running");
        Ok(state == KernelState::Running && services_running)
    }

    pub async fn get_recent_logs(&self, limit: usize) -> Result<Vec<LogDto>> {
        if let Some(log_service) = &self.log_service {
            let events: Vec<LogEvent> = log_service.recent_events(limit).await;
            Ok(events.into_iter().map(LogDto::from).collect())
        } else {
            Ok(Vec::new())
        }
    }

    pub async fn get_latency_summary(&self) -> Result<LatencySummaryDto> {
        if let Some(log_service) = &self.log_service {
            Ok(log_service.latency_summary().await.into())
        } else {
            Ok(LatencySummaryDto::default())
        }
    }

    pub async fn get_latency_histogram(&self) -> Result<Vec<LatencyBucketDto>> {
        if let Some(log_service) = &self.log_service {
            let buckets = log_service
                .latency_histogram()
                .await
                .into_iter()
                .map(LatencyBucketDto::from)
                .collect();
            Ok(buckets)
        } else {
            Ok(Vec::new())
        }
    }

    pub async fn get_positions(&self) -> Result<Vec<PositionDto>> {
        if let Some(exec) = &self.exec_service {
            let (balance, positions) = exec.positions().await;
            let mut out = Vec::new();
            for (symbol, size) in positions {
                let base = symbol
                    .chars()
                    .take_while(|c| c.is_alphabetic())
                    .collect::<String>();
                let quote = symbol
                    .chars()
                    .skip_while(|c| c.is_alphabetic())
                    .collect::<String>();
                out.push(PositionDto {
                    symbol: symbol.clone(),
                    base: base.clone(),
                    quote,
                    position_base: size,
                    balance_usdt: balance,
                });
            }
            Ok(out)
        } else {
            Ok(Vec::new())
        }
    }

    pub async fn get_cluster_nodes(&self) -> Result<Vec<ClusterNodeDto>> {
        if let Some(cluster) = &self.cluster_service {
            let nodes = cluster.nodes().await;
            Ok(nodes.into_values().map(ClusterNodeDto::from).collect())
        } else {
            Ok(Vec::new())
        }
    }

    pub async fn start_backtest(
        &self,
        exchange: ExchangeId,
        symbols: Vec<Symbol>,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
        speed_factor: f64,
    ) -> Result<()> {
        if let Some(manager) = &self.backtest {
            let config = BacktestConfig {
                exchange,
                symbols,
                start,
                end,
                speed_factor,
            };
            manager.start(config).await
        } else {
            anyhow::bail!("backtest coordinator not configured")
        }
    }

    pub async fn backtest_status(&self) -> Result<w2uos_backtest::BacktestStatus> {
        if let Some(manager) = &self.backtest {
            Ok(manager.status().await)
        } else {
            anyhow::bail!("backtest coordinator not configured")
        }
    }

    pub async fn risk_status(&self) -> Result<crate::types::RiskStatusDto> {
        if let Some(risk) = &self.risk_service {
            Ok(risk.status().into())
        } else {
            anyhow::bail!("risk supervisor not configured")
        }
    }

    pub async fn reset_circuit(&self) -> Result<()> {
        if let Some(risk) = &self.risk_service {
            risk.reset_circuit().await
        } else {
            anyhow::bail!("risk supervisor not configured")
        }
    }

    pub fn get_net_profile(&self) -> NetProfile {
        self.net_profile.clone()
    }
}
