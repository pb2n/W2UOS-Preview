//! Order execution and exchange adapters for W2UOS.

pub mod binance;
pub mod okx;
pub mod service;
pub mod types;

pub use binance::{BinanceCredentials, BinanceExecutionConfig};
pub use okx::{OkxCredentials, OkxExecutionConfig};
pub use service::{
    ExecutionBackend, ExecutionConfig, ExecutionKernelService, ExecutionService,
    PaperExecutionConfig,
};
pub use types::{OrderCommand, OrderResult, OrderSide, OrderStatus, OrderType};
