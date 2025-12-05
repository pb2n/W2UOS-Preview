//! Market data ingestion and normalization for W2UOS.

pub mod history;
pub mod okx;
pub mod service;
pub mod types;

pub use history::HistoricalStore;
pub use service::{
    DataMode, MarketDataConfig, MarketDataKernelService, MarketDataService, MarketDataSubscription,
    MarketHistoryConfig,
};
pub use types::{ExchangeId, MarketSnapshot, Symbol};
