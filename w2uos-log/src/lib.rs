//! Structured logging and storage for W2UOS.

pub mod service;
pub mod sink;
#[cfg(test)]
mod tests;
pub mod types;

pub use service::{log_event_via_bus, log_trade_via_bus, LogKernelService, LogService};
pub use sink::{CompositeLogSink, FileLogSink, LogSink, SqlLogSink};
pub use types::{
    ControlActionRecord, LogEvent, LogLevel, LogSource, TradeRecord, TradeSide, TradeStatus,
    TradeSymbol,
};
