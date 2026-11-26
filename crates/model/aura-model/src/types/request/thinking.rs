use serde::{Deserialize, Serialize};

/// Per-request extended thinking configuration.
///
/// When set on a `ModelRequest`, the provider will enable extended thinking
/// with the specified budget. When `None`, the provider may apply its own
/// heuristic (e.g., auto-enable for capable models).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingConfig {
    /// Token budget allocated for the thinking phase.
    /// Must be >= 1024 and < `max_tokens`.
    pub budget_tokens: u32,
}

/// Phase 2: explicit per-request reasoning effort knob, codex's
/// `reasoning.effort` analog (see
/// [codex-rs/core/src/client.rs:698-714](https://github.com/.../codex-rs/core/src/client.rs)).
///
/// Callers opt in by setting `ModelRequest::thinking_effort = Some(...)`.
/// **Backwards-compatibility note:** when `thinking_effort` is `None`, the
/// Anthropic provider falls through to the legacy `max_tokens > 2048`
/// auto-enable path so existing callers do not change behaviour during
/// rollout. New callers (e.g. the dev-loop) should set this explicitly.
///
/// Variants are declared from least to most thinking, so the derived
/// ordering compares levels (`Off < Minimal < … < Max`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ThinkingEffort {
    /// Extended thinking disabled for this request.
    Off,
    /// Lowest user-selectable tier. Aura Router maps it to `none` for
    /// current OpenAI models; for Anthropic it requests the smallest
    /// thinking budget (folding to `Low`-equivalent behaviour because
    /// Anthropic has no sub-`Low` tier).
    Minimal,
    /// Standard mode, ~1024-token budget. Fast tool calls without
    /// burning a multi-minute deliberation pass.
    Low,
    /// Adaptive mode, ~4096-token budget. Default for analysis turns.
    Medium,
    /// Adaptive mode with a budget proportional to `max_tokens`
    /// (clamped to `8192..=16000`). Use sparingly — this is the
    /// "burn a lot of thinking" knob.
    High,
    /// Maximum-leaning tier exposed to users via the chat model
    /// picker. In `enabled` mode it requests an even larger budget than
    /// [`Self::High`] (clamped to `16000..=24000`). Opus 5 maps this to
    /// native `output_config.effort = "xhigh"`; older adaptive models fold
    /// it into their highest supported value (`"high"`).
    XHigh,
    /// Top user-selectable tier. In `enabled` mode it requests the
    /// largest budget (clamped to `24000..=32000`). Opus 5 maps this to
    /// native `output_config.effort = "max"`; older adaptive models fold
    /// it into `output_config.effort = "high"`.
    Max,
}

impl ThinkingEffort {
    /// Parse the wire string sent by the chat model picker
    /// (`minimal`/`low`/`medium`/`high`/`max`). Case-insensitive.
    /// Returns `None` for unknown / empty input so callers fall back to
    /// their own heuristic. `xhigh` is accepted for backward
    /// compatibility and folds into [`Self::XHigh`].
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "minimal" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::XHigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }

    /// The level's lowercase name (`off`, `minimal`, `low`, `medium`,
    /// `high`, `xhigh`, `max`), as recorded with a model call and shown
    /// to the user. Unlike [`Self::reasoning_effort_wire`], `Off` has a
    /// name too, so a record can say thinking was turned off. Round-trips
    /// through [`Self::from_wire`].
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    /// Provider-neutral `reasoning_effort` wire string the harness puts
    /// on the outgoing request body for the router to translate into
    /// each provider's native control. `Off` carries no field
    /// (returns `None`).
    #[must_use]
    pub const fn reasoning_effort_wire(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Minimal => Some("minimal"),
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::XHigh => Some("xhigh"),
            Self::Max => Some("max"),
        }
    }
}
