//! Networking utilities and proxy-aware client builders for W2UOS.

pub mod client;
pub mod config;
pub mod traffic;

pub use client::build_http_client;
pub use config::{ConnectionReusePolicy, NetMode, NetProfile};
pub use traffic::{build_client_and_shaper, TrafficShaper};
