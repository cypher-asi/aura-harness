//! How much of a subscription plan's usage limit was used, as the
//! provider reports it in the response headers of a call made on the
//! plan. A [`PlanUsage`] is a reading of the whole account at the time of
//! the call, not what the call itself used.

use serde::{Deserialize, Serialize};

/// A subscription plan whose provider reports its usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Plan {
    /// Claude Pro or Max (`claude/*` routes).
    Claude,
    /// ChatGPT Plus or Pro (`chatgpt/*` routes).
    Chatgpt,
}

impl Plan {
    /// How the plan is named to the user.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude plan",
            Self::Chatgpt => "ChatGPT plan",
        }
    }
}

/// One usage window of a plan at the time of a call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanUsage {
    /// The plan.
    pub plan: Plan,
    /// The window: `5h`, `7d`, `7d_opus`, ...
    pub window: String,
    /// Fraction of the window's limit used, from 0 (above 1 when over
    /// the limit).
    pub used: f64,
    /// When the window resets, in Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<u64>,
}

impl PlanUsage {
    /// Whether `other` is a reading of the same window of the same plan.
    #[must_use]
    pub fn same_window(&self, other: &Self) -> bool {
        self.plan == other.plan && self.window == other.window
    }
}

/// The share of one plan window a run of a comparison used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanShare {
    /// The plan.
    pub plan: Plan,
    /// The window.
    pub window: String,
    /// The window's first reading among the run's calls.
    pub first: f64,
    /// Its last reading among them.
    pub last: f64,
    /// Fraction of the window's limit attributed to the run.
    pub used: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_usage_round_trips_and_omits_an_unknown_reset() {
        let reading = PlanUsage {
            plan: Plan::Chatgpt,
            window: "5h".to_string(),
            used: 0.125,
            resets_at: None,
        };
        let json = serde_json::to_value(&reading).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"plan": "chatgpt", "window": "5h", "used": 0.125})
        );
        assert_eq!(serde_json::from_value::<PlanUsage>(json).unwrap(), reading);
        assert_eq!(Plan::Claude.label(), "Claude plan");
    }
}
