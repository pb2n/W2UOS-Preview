use serde::{Deserialize, Serialize};
use w2uos_backtest::{BacktestResult, BacktestStatus};
use w2uos_kernel::NodeHeartbeat;
use w2uos_kernel::RiskStatus;
use w2uos_log::types::{LatencyBucket, LatencySummary, LogEvent, LogLevel, LogSource};
use w2uos_log::{ControlActionRecord, TradeRecord};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecentTradeDto {
    pub ts: String,
    pub side: String,
    pub price: f64,
    pub size_quote: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OrderbookSummaryDto {
    pub best_bid_size: f64,
    pub best_ask_size: f64,
    pub bid_depth_usdt: f64,
    pub ask_depth_usdt: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MarketMetricsDto {
    pub symbol: String,
    pub exchange: String,
    pub last_price: f64,
    pub bid: f64,
    pub ask: f64,
    pub spread_bp: f64,
    pub volume_24h: f64,
    pub funding_rate: Option<f64>,
    pub open_interest: Option<f64>,
    pub volatility_1h_pct: Option<f64>,
    pub volatility_24h_pct: Option<f64>,
    pub orderbook: OrderbookSummaryDto,
    pub recent_trades: Vec<RecentTradeDto>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MarketSymbolSummaryDto {
    pub symbol: String,
    pub last_price: f64,
    pub change_24h_pct: f64,
    pub volatility_24h_pct: f64,
    pub volume_24h: f64,
    pub in_watchlist: bool,
    pub has_open_position: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MarketSummaryResponseDto {
    pub exchange: String,
    pub symbols: Vec<MarketSymbolSummaryDto>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SystemMetricsDto {
    pub node: NodeInfoDto,
    pub runtime: RuntimeInfoDto,
    pub strategy: StrategyInfoDto,
    pub risk: RiskInfoDto,
    pub ibmq: IbmqInfoDto,
    pub x402: X402InfoDto,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NodeInfoDto {
    pub node_id: String,
    pub env: String,
    pub version: String,
    pub uptime_seconds: u64,
    pub hostname: String,
    pub trading_state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RuntimeInfoDto {
    pub cpu_load_pct: f64,
    pub mem_used_mb: f64,
    pub mem_total_mb: f64,
    pub disk_used_gb: f64,
    pub disk_total_gb: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StrategyInfoDto {
    pub active_strategies: Vec<String>,
    pub signals_per_minute: f64,
    pub avg_decision_latency_ms: f64,
    pub error_count_last_10m: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RiskInfoDto {
    pub risk_profile: String,
    pub max_leverage: i32,
    pub max_notional_usdt: f64,
    pub max_concurrent_positions: i32,
    pub current_notional_usdt: f64,
    pub open_positions_count: i32,
    pub intraday_drawdown_pct: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IbmqInfoDto {
    pub enabled: bool,
    pub role: String,
    pub last_job_ts: Option<String>,
    pub last_job_status: Option<String>,
    pub last_solution_score: Option<f64>,
    pub schedule_mode: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct X402InfoDto {
    pub enabled: bool,
    pub role: String,
    pub registered_agents: u32,
    pub pending_settlements: u32,
    pub last_settlement_ts: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceStatusDto {
    pub id: String,
    pub state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NodeStatusDto {
    pub kernel_state: String,
    pub kernel_mode: String,
    pub trading_mode: String,
    pub trading_state: String,
    pub node_id: String,
    pub env: String,
    pub armed_exchanges: Vec<String>,
    pub risk_profile: String,
    pub max_leverage: i32,
    pub max_notional_usdt: f64,
    pub max_concurrent_positions: i32,
    pub services: Vec<ServiceStatusDto>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BacktestRequestDto {
    pub exchange: String,
    pub symbols: Vec<String>,
    pub start: chrono::DateTime<chrono::Utc>,
    pub end: chrono::DateTime<chrono::Utc>,
    pub speed_factor: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BacktestStatusDto {
    pub running: bool,
    pub progress: f64,
    pub current_ts: Option<chrono::DateTime<chrono::Utc>>,
    pub result: Option<BacktestResultDto>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BacktestResultDto {
    pub pnl: f64,
    pub max_drawdown: f64,
    pub trade_count: usize,
    pub win_rate: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ClusterNodeDto {
    pub node_id: String,
    pub role: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub cpu_load_pct: f64,
    pub mem_load_pct: f64,
    pub strategy_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PositionDto {
    pub symbol: String,
    pub base: String,
    pub quote: String,
    pub position_base: f64,
    pub balance_usdt: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LiveStatusDto {
    pub exchange: String,
    pub mode: String,
    pub symbols: Vec<String>,
    pub market_connected: bool,
    pub last_snapshot_ts: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LiveOrderDto {
    pub id: String,
    pub ts: chrono::DateTime<chrono::Utc>,
    pub exchange: String,
    pub mode: String,
    pub symbol: String,
    pub side: String,
    pub size_quote: f64,
    pub price: f64,
    pub status: String,
    pub correlation_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RiskStatusDto {
    pub daily_pnl_quote: f64,
    pub total_notional: f64,
    pub open_positions: usize,
    pub circuit_breaker_active: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LogDto {
    pub ts: chrono::DateTime<chrono::Utc>,
    pub level: LogLevel,
    pub source: LogSource,
    pub message: String,
    pub fields: serde_json::Value,
    pub correlation_id: Option<String>,
    pub trace_id: Option<String>,
}

impl From<LogEvent> for LogDto {
    fn from(value: LogEvent) -> Self {
        Self {
            ts: value.ts,
            level: value.level,
            source: value.source,
            message: value.message,
            fields: value.fields,
            correlation_id: value.correlation_id,
            trace_id: value.trace_id.map(|t| t.0),
        }
    }
}

impl From<TradeRecord> for LiveOrderDto {
    fn from(value: TradeRecord) -> Self {
        Self {
            id: value.id,
            ts: value.ts,
            exchange: value.exchange,
            mode: value.mode,
            symbol: format!("{}-{}", value.symbol.base, value.symbol.quote),
            side: format!("{:?}", value.side),
            size_quote: value.size_quote,
            price: value.price,
            status: String::from(&value.status),
            correlation_id: value.correlation_id,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct LatencySummaryDto {
    pub min_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub p95_ms: Option<f64>,
    pub p99_ms: Option<f64>,
}

impl From<LatencySummary> for LatencySummaryDto {
    fn from(value: LatencySummary) -> Self {
        Self {
            min_ms: value.min_ms,
            avg_ms: value.avg_ms,
            p95_ms: value.p95_ms,
            p99_ms: value.p99_ms,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LatencyBucketDto {
    pub upper_bound_ms: i64,
    pub count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ControlResponseDto {
    pub ok: bool,
    pub state: String,
    pub message: String,
    pub audit_id: String,
}

impl ControlResponseDto {
    pub fn new(
        ok: bool,
        state: w2uos_kernel::TradingState,
        message: &str,
        audit_id: String,
    ) -> Self {
        Self {
            ok,
            state: state.as_str().to_string(),
            message: message.to_string(),
            audit_id,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ControlActionDto {
    pub id: Option<i64>,
    pub audit_id: String,
    pub ts: chrono::DateTime<chrono::Utc>,
    pub actor: String,
    pub action: String,
    pub params: serde_json::Value,
    pub previous_state: Option<String>,
    pub new_state: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RiskProfileResponseDto {
    pub ok: bool,
    pub risk_profile: String,
    pub max_leverage: i32,
    pub max_notional_usdt: f64,
    pub max_concurrent_positions: i32,
    pub message: String,
    pub audit_id: String,
}

impl From<LatencyBucket> for LatencyBucketDto {
    fn from(value: LatencyBucket) -> Self {
        Self {
            upper_bound_ms: value.upper_bound_ms,
            count: value.count,
        }
    }
}

impl From<NodeHeartbeat> for ClusterNodeDto {
    fn from(value: NodeHeartbeat) -> Self {
        Self {
            node_id: value.node_id.0,
            role: value.role.to_string(),
            timestamp: value.timestamp,
            cpu_load_pct: value.cpu_load_pct,
            mem_load_pct: value.mem_load_pct,
            strategy_count: value.strategy_count,
        }
    }
}

impl From<ControlActionRecord> for ControlActionDto {
    fn from(value: ControlActionRecord) -> Self {
        Self {
            id: value.id,
            audit_id: value.audit_id,
            ts: value.ts,
            actor: value.actor,
            action: value.action,
            params: value.params,
            previous_state: value.previous_state,
            new_state: value.new_state,
        }
    }
}

impl From<BacktestResult> for BacktestResultDto {
    fn from(value: BacktestResult) -> Self {
        Self {
            pnl: value.pnl,
            max_drawdown: value.max_drawdown,
            trade_count: value.trade_count,
            win_rate: value.win_rate,
        }
    }
}

impl From<BacktestStatus> for BacktestStatusDto {
    fn from(value: BacktestStatus) -> Self {
        Self {
            running: value.running,
            progress: value.progress,
            current_ts: value.current_ts,
            result: value.result.map(BacktestResultDto::from),
        }
    }
}

impl From<RiskStatus> for RiskStatusDto {
    fn from(value: RiskStatus) -> Self {
        Self {
            daily_pnl_quote: value.daily_pnl_quote,
            total_notional: value.total_notional,
            open_positions: value.open_positions,
            circuit_breaker_active: value.circuit_breaker_active,
        }
    }
}
