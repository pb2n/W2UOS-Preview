use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use w2uos_service::TraceId;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LogSource {
    Kernel,
    MarketData,
    Execution,
    Strategy,
    Api,
    Other(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEvent {
    pub ts: DateTime<Utc>,
    pub level: LogLevel,
    pub source: LogSource,
    pub message: String,
    pub fields: serde_json::Value,
    pub correlation_id: Option<String>,
    pub trace_id: Option<TraceId>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TradeStatus {
    Filled,
    Rejected(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TradeSide {
    Buy,
    Sell,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TradeRecord {
    pub id: String,
    pub ts: DateTime<Utc>,
    pub exchange: String,
    pub symbol: TradeSymbol,
    pub side: TradeSide,
    pub size_quote: f64,
    pub price: f64,
    pub status: TradeStatus,
    pub strategy_id: Option<String>,
    pub correlation_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TradeSymbol {
    pub base: String,
    pub quote: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LatencyRecord {
    pub trace_id: TraceId,
    pub ts_start_tick: DateTime<Utc>,
    pub ts_strategy_decision: DateTime<Utc>,
    pub ts_order_result: DateTime<Utc>,
    pub tick_to_strategy_ms: i64,
    pub strategy_to_exec_ms: i64,
    pub tick_to_result_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct LatencySummary {
    pub min_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub p95_ms: Option<f64>,
    pub p99_ms: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct LatencyBucket {
    pub upper_bound_ms: i64,
    pub count: usize,
}

impl From<&TradeStatus> for String {
    fn from(status: &TradeStatus) -> Self {
        match status {
            TradeStatus::Filled => "Filled".to_string(),
            TradeStatus::Rejected(reason) => format!("Rejected({})", reason),
        }
    }
}
