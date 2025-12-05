use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GlobalRiskConfig {
    pub max_daily_loss_pct: f64,
    pub max_daily_loss_abs: f64,
    pub max_total_notional: f64,
    pub max_open_positions: usize,
    pub max_error_rate_per_minute: Option<f64>,
}

impl Default for GlobalRiskConfig {
    fn default() -> Self {
        Self {
            max_daily_loss_pct: 5.0,
            max_daily_loss_abs: 10_000.0,
            max_total_notional: 250_000.0,
            max_open_positions: 25,
            max_error_rate_per_minute: Some(10.0),
        }
    }
}
