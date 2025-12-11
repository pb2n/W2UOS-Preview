use serde::{Deserialize, Serialize};
use w2uos_data::{ExchangeId, Symbol};
use w2uos_service::TraceId;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionMode {
    Sim,
    Paper,
    Live,
}

impl Default for ExecutionMode {
    fn default() -> Self {
        ExecutionMode::Sim
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum OrderType {
    Market,
    Limit,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderCommand {
    pub id: String,
    pub exchange: ExchangeId,
    pub symbol: Symbol,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub size_quote: f64,
    pub limit_price: Option<f64>,
    pub trace_id: Option<TraceId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum OrderStatus {
    New,
    Filled,
    Rejected(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderResult {
    pub command_id: String,
    pub exchange: ExchangeId,
    pub symbol: Symbol,
    pub side: OrderSide,
    pub status: OrderStatus,
    pub filled_size_quote: f64,
    pub avg_price: f64,
    pub ts: chrono::DateTime<chrono::Utc>,
    pub trace_id: Option<TraceId>,
}
