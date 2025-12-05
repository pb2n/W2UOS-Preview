pub mod types;

use anyhow::Result;
use w2uos_data::MarketSnapshot;

pub use types::{BotConfig, PairId, PlannedOrder, StrategyOutcome};

#[derive(Clone)]
pub struct StrategyRuntime {
    config: BotConfig,
}

impl StrategyRuntime {
    pub fn new(config: &BotConfig) -> Result<Self> {
        Ok(Self {
            config: config.clone(),
        })
    }

    pub fn on_market_snapshot(
        &mut self,
        _pair: &PairId,
        _snap: &MarketSnapshot,
    ) -> StrategyOutcome {
        // Placeholder runtime: no decisions by default.
        StrategyOutcome {
            planned_orders: Vec::new(),
            notes: None,
            trace_id: None,
        }
    }

    pub fn planned_pairs(&self) -> &[PairId] {
        &self.config.pairs
    }
}
