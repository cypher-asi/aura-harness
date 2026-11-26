//! Tokens and cost of model calls, per turn and per session.

use serde::{Deserialize, Serialize};

/// Tokens of a turn that one sign-in paid for: `Claude plan`,
/// `OpenRouter`, `aura-router`, `ChatGPT plan` or `local`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceUsage {
    /// Who paid, as the footer names it
    pub label: String,
    /// Whole prompt (cached or not) plus output of the calls it paid for
    pub tokens: u64,
    /// Estimated cost of those calls at list prices; `None` when none was priced
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Calls with no known price, which `cost_usd` leaves out
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unpriced_calls: u32,
}

/// Tokens model calls used and what they cost.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CallUsage {
    /// Whole prompt (cached or not) plus output
    pub tokens: u64,
    /// Estimated cost at API list prices; `None` for local or unpriced models
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Calls with no known price, which `cost_usd` leaves out. Local
    /// models are free, not unpriced.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unpriced_calls: u32,
}

/// Whether a count is 0, so it is left out when serialized.
#[allow(clippy::trivially_copy_pass_by_ref)] // serde passes a reference
const fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Tokens and cost of one model call, for the session totals.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallStats {
    /// Model the call went to
    pub model: String,
    /// Prompt tokens neither read from nor written to the cache
    pub input_tokens: u64,
    /// Prompt tokens read from the cache
    pub cache_read_tokens: u64,
    /// Prompt tokens written to the cache
    pub cache_write_tokens: u64,
    /// Output tokens
    pub output_tokens: u64,
    /// Estimated cost at API list prices; `None` for local or unpriced models
    pub cost_usd: Option<f64>,
    /// Whether the model has no known price, so `cost_usd` leaves the
    /// call out. Local models are free, not unpriced.
    pub unpriced: bool,
    /// Context window of the model, when known
    pub context_limit: Option<u64>,
}

impl CallStats {
    /// The whole prompt: what the call put in the context window.
    #[must_use]
    pub const fn prompt_tokens(&self) -> u64 {
        self.input_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

/// Usage of a session so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionTotals {
    /// Model calls made
    pub calls: u32,
    /// Prompt tokens neither read from nor written to the cache
    pub input_tokens: u64,
    /// Prompt tokens read from the cache
    pub cache_read_tokens: u64,
    /// Prompt tokens written to the cache
    pub cache_write_tokens: u64,
    /// Output tokens
    pub output_tokens: u64,
    /// Estimated cost; `None` when no call had a price
    pub cost_usd: Option<f64>,
    /// Calls with no known price, which `cost_usd` leaves out
    pub unpriced_calls: u32,
    /// Prompt of the latest call
    pub context_tokens: u64,
    /// Models the calls went to, first use first
    pub models: Vec<String>,
}
