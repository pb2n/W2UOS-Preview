use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use tokio::sync::{Notify, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration};
use tracing::{info, warn};
use w2uos_bus::{BusMessage, MessageBus};
use w2uos_service::{Service, ServiceId};

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct NodeId(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NodeRole {
    Data,
    Exec,
    Strategy,
    AllInOne,
}

impl Default for NodeRole {
    fn default() -> Self {
        NodeRole::AllInOne
    }
}

impl std::fmt::Display for NodeRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                NodeRole::Data => "data",
                NodeRole::Exec => "exec",
                NodeRole::Strategy => "strategy",
                NodeRole::AllInOne => "all-in-one",
            }
        )
    }
}

impl FromStr for NodeRole {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "data" => Ok(NodeRole::Data),
            "exec" => Ok(NodeRole::Exec),
            "strategy" => Ok(NodeRole::Strategy),
            "all" | "all-in-one" | "allinone" => Ok(NodeRole::AllInOne),
            other => Err(anyhow!("unknown node role: {}", other)),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NodeHeartbeat {
    pub node_id: NodeId,
    pub role: NodeRole,
    pub timestamp: DateTime<Utc>,
    pub cpu_load_pct: f64,
    pub mem_load_pct: f64,
    pub strategy_count: usize,
}

#[derive(Clone)]
pub struct ClusterService {
    id: ServiceId,
    node_id: NodeId,
    role: NodeRole,
    bus: Arc<dyn MessageBus>,
    heartbeats: Arc<RwLock<HashMap<NodeId, NodeHeartbeat>>>,
    shutdown: Arc<Notify>,
    worker: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl ClusterService {
    pub fn new(id: ServiceId, node_id: NodeId, role: NodeRole, bus: Arc<dyn MessageBus>) -> Self {
        Self {
            id,
            node_id,
            role,
            bus,
            heartbeats: Arc::new(RwLock::new(HashMap::new())),
            shutdown: Arc::new(Notify::new()),
            worker: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn nodes(&self) -> HashMap<NodeId, NodeHeartbeat> {
        self.heartbeats.read().await.clone()
    }

    async fn run(self: Arc<Self>) {
        let bus_for_rx = Arc::clone(&self.bus);
        let heartbeats = Arc::clone(&self.heartbeats);
        let shutdown_rx = self.shutdown.clone();

        let recv_task = tokio::spawn(async move {
            match bus_for_rx.subscribe("cluster.heartbeat").await {
                Ok(mut sub) => loop {
                    tokio::select! {
                        _ = shutdown_rx.notified() => break,
                        msg = sub.receiver.recv() => {
                            if let Some(msg) = msg {
                                match serde_json::from_slice::<NodeHeartbeat>(&msg.0) {
                                    Ok(hb) => {
                                        heartbeats.write().await.insert(hb.node_id.clone(), hb);
                                    }
                                    Err(err) => warn!(?err, "failed to decode heartbeat"),
                                }
                            } else {
                                break;
                            }
                        }
                    }
                },
                Err(err) => warn!(?err, "cluster service failed to subscribe to heartbeats"),
            }
        });

        let bus_for_tx = Arc::clone(&self.bus);
        let node_id = self.node_id.clone();
        let role = self.role;
        let heartbeats = Arc::clone(&self.heartbeats);
        let shutdown_tx = self.shutdown.clone();

        let send_task = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(5));
            loop {
                tokio::select! {
                    _ = shutdown_tx.notified() => break,
                    _ = ticker.tick() => {
                        let heartbeat = NodeHeartbeat {
                            node_id: node_id.clone(),
                            role,
                            timestamp: Utc::now(),
                            cpu_load_pct: 0.0,
                            mem_load_pct: 0.0,
                            strategy_count: 0,
                        };

                        heartbeats.write().await.insert(node_id.clone(), heartbeat.clone());

                        match serde_json::to_vec(&heartbeat) {
                            Ok(payload) => {
                                if let Err(err) = bus_for_tx
                                    .publish("cluster.heartbeat", BusMessage(payload))
                                    .await
                                {
                                    warn!(?err, "failed to publish heartbeat");
                                }
                            }
                            Err(err) => warn!(?err, "failed to serialize heartbeat"),
                        }
                    }
                }
            }
        });

        let _ = tokio::join!(recv_task, send_task);
    }
}

#[async_trait::async_trait]
impl Service for ClusterService {
    fn id(&self) -> &ServiceId {
        &self.id
    }

    async fn start(&self) -> Result<()> {
        let mut guard = self.worker.lock().unwrap();
        if guard.is_some() {
            return Ok(());
        }

        let worker = {
            let svc = Arc::new(self.clone());
            tokio::spawn(async move { svc.run().await })
        };

        *guard = Some(worker);
        info!(service = %self.id, %self.role, node = %self.node_id.0, "cluster service started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.shutdown.notify_waiters();
        if let Some(handle) = self.worker.lock().unwrap().take() {
            handle.abort();
        }
        info!(service = %self.id, node = %self.node_id.0, "cluster service stopped");
        Ok(())
    }

    async fn health_check(&self) -> Result<()> {
        let guard = self.worker.lock().unwrap();
        if let Some(handle) = guard.as_ref() {
            if handle.is_finished() {
                return Err(anyhow!("cluster worker finished unexpectedly"));
            }
            Ok(())
        } else {
            Err(anyhow!("cluster worker not running"))
        }
    }
}
