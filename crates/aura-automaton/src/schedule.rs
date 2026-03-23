use serde::{Deserialize, Serialize};

/// Declarative schedule shape for automata (serialization / config / tooling).
///
/// The runtime currently drives work with a tight tick loop and does **not** enforce
/// these variants (no interval sleeps, cron parsing, or event gating in the loop).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Schedule {
    Continuous,
    Interval { seconds: u64 },
    Cron { expression: String },
    OnDemand,
    EventDriven { event_filter: String },
}

impl Schedule {
    pub fn is_continuous(&self) -> bool {
        matches!(self, Self::Continuous)
    }

    pub fn is_on_demand(&self) -> bool {
        matches!(self, Self::OnDemand)
    }
}
