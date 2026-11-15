//! Cost gate settings: where its policy and ledger live, how much it
//! explores, and tier overrides.

use std::path::{Path, PathBuf};

use crate::env::{
    lookup_string, AURA_GATE_EXPLORE, AURA_GATE_EXPLORE_BUDGET_USD, AURA_GATE_LEDGER,
    AURA_GATE_POLICY, AURA_GATE_TIERS,
};

/// Directory under `AURA_HOME` holding the gate's policy and ledger.
pub const GATE_DIR: &str = "gate";

/// Active policy file name under [`GATE_DIR`].
pub const POLICY_FILE: &str = "policy.json";

/// Attempt ledger file name under [`GATE_DIR`].
pub const LEDGER_FILE: &str = "ledger.jsonl";

/// Cost gate settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GateConfig {
    /// env: `AURA_GATE_POLICY` (default: `AURA_HOME/gate/policy.json`
    /// when it exists, else the built-in rules)
    pub policy: Option<PathBuf>,
    /// env: `AURA_GATE_EXPLORE` (default: the policy's own rate)
    pub explore_rate: Option<f64>,
    /// env: `AURA_GATE_EXPLORE_BUDGET_USD` (default: 1.0)
    pub explore_budget_usd: Option<f64>,
    /// env: `AURA_GATE_TIERS`
    pub tiers: Option<String>,
    /// env: `AURA_GATE_LEDGER=off` disables it (default:
    /// `AURA_HOME/gate/ledger.jsonl`)
    pub ledger: Option<PathBuf>,
}

impl GateConfig {
    /// Read the env, resolving default paths under `home`.
    #[must_use]
    pub fn from_env(home: Option<&Path>) -> Self {
        let dir = home.map(|h| h.join(GATE_DIR));
        let policy = lookup_string(AURA_GATE_POLICY).map(PathBuf::from).or_else(|| {
            dir.as_ref()
                .map(|d| d.join(POLICY_FILE))
                .filter(|p| p.is_file())
        });
        let rate = |name| {
            lookup_string(name)
                .and_then(|raw| raw.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.0)
        };
        let ledger_off = lookup_string(AURA_GATE_LEDGER)
            .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "off" | "0" | "false"));
        Self {
            policy,
            explore_rate: rate(AURA_GATE_EXPLORE).map(|r| r.min(1.0)),
            explore_budget_usd: rate(AURA_GATE_EXPLORE_BUDGET_USD),
            tiers: lookup_string(AURA_GATE_TIERS),
            ledger: if ledger_off {
                None
            } else {
                dir.map(|d| d.join(LEDGER_FILE))
            },
        }
    }
}
