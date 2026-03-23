use serde::{Deserialize, Serialize};

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
