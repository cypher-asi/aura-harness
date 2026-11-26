//! Compaction tiers, tier-selection policy, and the reports a pass returns.

use aura_model::{Message, ModelRequestKind};

use super::summary::SummaryInput;

const DEFAULT_SUMMARY_AT: f64 = 0.85;

/// Compaction tier configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionConfig {
    /// Maximum characters for tool results in older messages.
    pub tool_result_max_chars: usize,
    /// Maximum characters for plain text in older messages.
    pub text_max_chars: usize,
    /// Number of recent messages to preserve uncompacted.
    pub preserve_recent: usize,
}

impl CompactionConfig {
    /// Emergency tier: very aggressive truncation for near-limit contexts
    /// (>=85%) and for overflow recovery.
    #[must_use]
    pub const fn emergency() -> Self {
        Self {
            tool_result_max_chars: 200,
            text_max_chars: 400,
            preserve_recent: 2,
        }
    }

    /// Aggressive tier: significant truncation for high-utilization contexts (>=70%).
    #[must_use]
    pub const fn aggressive() -> Self {
        Self {
            tool_result_max_chars: 500,
            text_max_chars: 800,
            preserve_recent: 4,
        }
    }

    /// Moderate tier: balanced truncation at medium-high utilization (>=60%).
    #[must_use]
    pub const fn moderate() -> Self {
        Self {
            tool_result_max_chars: 1000,
            text_max_chars: 1500,
            preserve_recent: 6,
        }
    }

    /// Light tier: gentle truncation for moderate utilization (>=30%).
    #[must_use]
    pub const fn light() -> Self {
        Self {
            tool_result_max_chars: 1500,
            text_max_chars: 2000,
            preserve_recent: 6,
        }
    }

    /// Minimal tier: the lightest truncation, preserving most context
    /// (>=15%).
    #[must_use]
    pub const fn minimal() -> Self {
        Self {
            tool_result_max_chars: 3000,
            text_max_chars: 4000,
            preserve_recent: 8,
        }
    }
}

/// Request policy used when choosing a message-compaction tier.
#[derive(Debug, Clone, Copy)]
pub struct CompactionPolicy {
    /// Model context window in tokens. `None` disables utilization-based selection.
    pub max_context_tokens: Option<u64>,
    /// Latest context estimate in tokens before output-reserve pressure is applied.
    pub estimated_context_tokens: u64,
    /// Current message-context estimate in tokens, when tracked separately.
    pub current_context_tokens: Option<u64>,
    /// Response token reserve included in pressure calculations.
    pub reserved_output_tokens: u64,
    /// Raw message bytes/chars used as a proxy-envelope pressure signal.
    pub raw_message_bytes: Option<usize>,
    /// Request contract kind, used to apply known body-size expectations.
    pub request_kind: Option<ModelRequestKind>,
    /// Explicit request body cap, when the caller knows one.
    pub request_body_cap_bytes: Option<usize>,
    /// Pressure at which local compaction asks the caller for model-backed summary escalation.
    pub summary_at: f64,
}

impl CompactionPolicy {
    /// Build the policy used by the agent loop from existing token estimates.
    #[must_use]
    pub fn new(
        max_context_tokens: Option<u64>,
        estimated_context_tokens: u64,
        reserved_output_tokens: u64,
    ) -> Self {
        Self {
            max_context_tokens,
            estimated_context_tokens,
            reserved_output_tokens,
            ..Self::default()
        }
    }

    const fn default_values() -> Self {
        Self {
            max_context_tokens: None,
            estimated_context_tokens: 0,
            current_context_tokens: None,
            reserved_output_tokens: 0,
            raw_message_bytes: None,
            request_kind: None,
            request_body_cap_bytes: None,
            summary_at: DEFAULT_SUMMARY_AT,
        }
    }
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        let mut policy = Self::default_values();
        policy.summary_at = env_unit_f64("AURA_COMPACTION_SUMMARY_AT", DEFAULT_SUMMARY_AT);
        policy
    }
}

fn env_unit_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
        .unwrap_or(default)
}

/// Mutable input bundle for message compaction.
pub struct CompactionInput<'a> {
    /// Messages to compact in place.
    pub messages: &'a mut [Message],
    /// Tier-selection policy.
    pub policy: CompactionPolicy,
}

/// Report returned by message compaction operations.
#[derive(Debug, Clone)]
pub struct CompactionReport {
    /// Character estimate before compaction.
    pub before_chars: usize,
    /// Character estimate after compaction.
    pub after_chars: usize,
    /// Action selected for this compaction pass.
    pub action: CompactionAction,
}

impl CompactionReport {
    /// Whether the operation reduced the estimated message footprint.
    #[must_use]
    pub const fn reduced(&self) -> bool {
        self.after_chars < self.before_chars
    }
}

/// Message-compaction action selected for a pass.
#[derive(Debug, Clone)]
pub enum CompactionAction {
    /// No compaction tier was selected.
    None,
    /// A tier was selected and applied.
    Applied(CompactionConfig),
    /// Local compaction ran, but the caller should ask a model to summarize middle history.
    NeedsSummary(SummaryInput),
}

/// Overflow recovery step used by the existing retry sequence.
#[derive(Debug, Clone, Copy)]
pub struct OverflowStep {
    /// Tier to apply before retrying.
    pub tier: CompactionConfig,
    /// User-facing warning emitted by the caller.
    pub warning: &'static str,
}

impl OverflowStep {
    /// Existing first recovery step.
    pub const AGGRESSIVE: Self = Self {
        tier: CompactionConfig::aggressive(),
        warning:
            "Context limit reached; compacting older context, trimming response budget, and retrying.",
    };

    /// Existing emergency recovery step.
    pub const EMERGENCY: Self = Self {
        tier: CompactionConfig::emergency(),
        warning:
            "Context is still too large; applying emergency compaction, trimming response budget again, and retrying.",
    };
}
