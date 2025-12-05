//! Order execution and exchange adapters for W2UOS.

pub mod okx;
pub mod service;
pub mod types;

pub use okx::{OkxCredentials, OkxExecutionConfig};
pub use service::{ExecutionConfig, ExecutionKernelService, ExecutionMode, ExecutionService};
pub use types::{OrderCommand, OrderResult, OrderSide, OrderStatus, OrderType};
