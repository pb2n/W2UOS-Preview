use serde::{Deserialize, Serialize};
use w2uos_service::TraceId;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExchangeId {
    Okx,
    Binance,
    Other(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TradingMode {
    Simulated,
    LiveOkx,
    LiveBinance,
}

impl Default for TradingMode {
    fn default() -> Self {
        TradingMode::Simulated
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Symbol {
    pub base: String,
    pub quote: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub ts: chrono::DateTime<chrono::Utc>,
    pub exchange: ExchangeId,
    pub symbol: Symbol,
    pub last: f64,
    pub bid: f64,
    pub ask: f64,
    pub volume_24h: f64,
    pub trace_id: Option<TraceId>,
}

impl std::fmt::Display for ExchangeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExchangeId::Okx => write!(f, "OKX"),
            ExchangeId::Binance => write!(f, "Binance"),
            ExchangeId::Other(name) => write!(f, "{}", name),
        }
    }
}

impl From<&str> for ExchangeId {
    fn from(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "okx" => ExchangeId::Okx,
            "binance" => ExchangeId::Binance,
            other => ExchangeId::Other(other.to_string()),
        }
    }
}
