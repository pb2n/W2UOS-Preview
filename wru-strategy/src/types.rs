use serde::{Deserialize, Serialize};
use w2uos_data::{ExchangeId, Symbol};
use w2uos_exec::{OrderSide, OrderType};
use w2uos_service::TraceId;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairId {
    pub exchange: ExchangeId,
    pub symbol: Symbol,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PlannedOrder {
    pub side: OrderSide,
    pub order_type: OrderType,
    pub size_quote: f64,
    pub limit_price: Option<f64>,
    pub trace_id: Option<TraceId>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StrategyOutcome {
    pub planned_orders: Vec<PlannedOrder>,
    pub notes: Option<String>,
    pub trace_id: Option<TraceId>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BotConfig {
    pub name: String,
    pub pairs: Vec<PairId>,
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            name: "default-bot".to_string(),
            pairs: Vec::new(),
        }
    }
}
