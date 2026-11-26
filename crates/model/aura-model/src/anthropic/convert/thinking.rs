//! Extended-thinking and `output_config.effort` settings per model.

use super::capability_model;
use crate::anthropic::api_types::{ApiOutputConfig, ApiThinkingConfig};
use crate::{ModelRequest, ThinkingEffort};

/// Resolve extended thinking config for a given model.
///
/// Uses the caller-supplied config when present; otherwise auto-enables
/// thinking for capable models when the token budget is large enough.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThinkingMode {
    Adaptive,
    Enabled,
}

fn normalize_anthropic_model(model: &str) -> String {
    capability_model(model)
        .trim()
        .to_ascii_lowercase()
        .trim_start_matches("aura-")
        .replace('.', "-")
}

fn thinking_mode_for_model(model: &str) -> Option<ThinkingMode> {
    let model = normalize_anthropic_model(model);
    if model.starts_with("claude-opus-5")
        || model.starts_with("claude-fable-5")
        || model.starts_with("claude-mythos-5")
        || model.starts_with("claude-sonnet-5")
        || model.starts_with("claude-haiku-5")
        || model.starts_with("claude-opus-4")
        || model.starts_with("claude-sonnet-4")
    {
        Some(ThinkingMode::Adaptive)
    } else if model.starts_with("claude-3-7-sonnet") {
        Some(ThinkingMode::Enabled)
    } else {
        None
    }
}

/// Whether `model` thinks when a request leaves `thinking` out, so turning
/// thinking off takes `{"type": "disabled"}`. Haiku 5 does: a request that
/// omits it can spend a small `max_tokens` on thinking and return no text.
/// Sonnet 5.5 rejects `disabled` (it wants `between_tools`) and Opus 5.5
/// has no off switch, so neither is listed.
fn thinks_unless_disabled(model: &str) -> bool {
    normalize_anthropic_model(model).starts_with("claude-haiku-5")
}

pub(in crate::anthropic) fn resolve_thinking(
    request: &ModelRequest,
    model: &str,
) -> Option<ApiThinkingConfig> {
    if request.thinking_effort == Some(ThinkingEffort::Off) && thinks_unless_disabled(model) {
        return Some(ApiThinkingConfig {
            thinking_type: "disabled".to_string(),
            budget_tokens: None,
            display: None,
        });
    }
    let mut thinking = resolve_thinking_unbounded(request, model)?;
    // Opus 4.7+ and Claude 5 default `display` to "omitted", which streams
    // empty thinking blocks. Always ask for the summary so the UI and the
    // transcript have reasoning text to show.
    thinking.display = Some("summarized");
    if let Some(budget) = thinking.budget_tokens {
        // Enabled thinking requires at least 1024 tokens and strictly less
        // than max_tokens. Small response caps must not produce invalid API requests.
        let ceiling = request.max_tokens.get().checked_sub(1)?;
        if ceiling < 1024 {
            return None;
        }
        // Leave response space for text/tool arguments on enabled-thinking models.
        let thinking_ceiling = (request.max_tokens.get() / 2).max(1024).min(ceiling);
        thinking.budget_tokens = Some(budget.min(thinking_ceiling));
    }
    Some(thinking)
}

fn resolve_thinking_unbounded(request: &ModelRequest, model: &str) -> Option<ApiThinkingConfig> {
    // Historical note: dev-loop requests used to escalate
    // [`ThinkingMode::Adaptive`] to [`ThinkingMode::Enabled`] to coax
    // visible `ThinkingDelta` frames out of opus-4 / sonnet-4, gated by
    // the `AURA_DEV_LOOP_ENABLED_THINKING` kill switch. Anthropic
    // removed `enabled` mode for the Claude 4 family in May 2026 —
    // requests now 400 with `"thinking.type.enabled" is not supported
    // for this model. Use "thinking.type.adaptive" and
    // "output_config.effort" to control thinking behavior.` Adaptive
    // plus `output_config.effort: "high"` (set by
    // `resolve_output_config` below) is the replacement, so the
    // escalation and its kill switch were dropped.
    let thinking_mode = thinking_mode_for_model(model)?;
    let max_tokens = request.max_tokens.get();

    // Phase 2: explicit reasoning-effort knob takes precedence over the
    // legacy `max_tokens > 2048` auto-enable path. Codex sets
    // `reasoning.effort` per request (codex-rs/core/src/client.rs:698-714);
    // when a caller opts in via `ModelRequest::thinking_effort = Some(_)`,
    // bypass both the explicit `ThinkingConfig` honoring AND the
    // max_tokens-coupled heuristic, and use the calibrated budget that
    // matches the requested effort. `None` falls through to the legacy
    // behaviour below so non-migrated callers keep their current shape.
    if let Some(effort) = request.thinking_effort {
        if effort == ThinkingEffort::Off {
            return None;
        }
        return Some(thinking_config(thinking_mode, || {
            enabled_budget_for_effort(effort, max_tokens)
        }));
    }

    if let Some(ref cfg) = request.thinking {
        return Some(thinking_config(thinking_mode, || cfg.budget_tokens));
    }

    (max_tokens > 2048)
        .then(|| thinking_config(thinking_mode, || (max_tokens / 2).clamp(1024, 16000)))
}

/// Thinking config for `mode`. Anthropic's `adaptive` thinking mode
/// rejects `budget_tokens` outright (`thinking.adaptive.budget_tokens:
/// Extra inputs are not permitted`); the model picks its own budget. Only
/// `enabled` mode (Claude 3.7) accepts a budget, which `enabled_budget`
/// computes.
fn thinking_config(mode: ThinkingMode, enabled_budget: impl FnOnce() -> u32) -> ApiThinkingConfig {
    ApiThinkingConfig {
        thinking_type: thinking_mode_label(mode).to_string(),
        display: None,
        budget_tokens: match mode {
            ThinkingMode::Adaptive => None,
            ThinkingMode::Enabled => Some(enabled_budget()),
        },
    }
}

/// `enabled`-mode thinking budget for an explicit `effort` (never
/// [`ThinkingEffort::Off`], which sends no thinking config).
fn enabled_budget_for_effort(effort: ThinkingEffort, max_tokens: u32) -> u32 {
    match effort {
        // Anthropic has no sub-`Low` thinking tier, so `Minimal`
        // requests the smallest budget the API accepts (in
        // `enabled` mode); in `adaptive` mode the model still picks
        // its own budget. The distinct `minimal` value is preserved
        // end-to-end for OpenAI via the neutral `reasoning_effort`
        // field on the outgoing body.
        ThinkingEffort::Off | ThinkingEffort::Minimal | ThinkingEffort::Low => 1024,
        ThinkingEffort::Medium => 4096,
        ThinkingEffort::High => (max_tokens / 2).clamp(8192, 16000),
        ThinkingEffort::XHigh => (max_tokens * 3 / 4).clamp(16000, 24000),
        ThinkingEffort::Max => max_tokens.clamp(24000, 32000),
    }
}

fn thinking_mode_label(mode: ThinkingMode) -> &'static str {
    match mode {
        ThinkingMode::Adaptive => "adaptive",
        ThinkingMode::Enabled => "enabled",
    }
}

fn supports_native_adaptive_effort_ladder(model: &str) -> bool {
    let model = normalize_anthropic_model(model);
    model.starts_with("claude-opus-5")
        || model.starts_with("claude-opus-4-7")
        || model.starts_with("claude-opus-4-8")
        || model.starts_with("claude-fable-5")
        || model.starts_with("claude-mythos-5")
        || model.starts_with("claude-sonnet-5")
}

pub(in crate::anthropic) fn resolve_output_config(
    request: &ModelRequest,
    model: &str,
) -> Option<ApiOutputConfig> {
    let thinking = resolve_thinking(request, model)?;
    if thinking.thinking_type != "adaptive" {
        return None;
    }
    adaptive_effort(model, request.thinking_effort).map(|effort| ApiOutputConfig {
        effort: effort.to_string(),
    })
}

/// The `output_config.effort` value an adaptive-thinking `model` is sent
/// for the `requested` effort, or `None` to send no `output_config`.
fn adaptive_effort(model: &str, requested: Option<ThinkingEffort>) -> Option<&'static str> {
    let effort = requested.unwrap_or(ThinkingEffort::High);
    if supports_native_adaptive_effort_ladder(model) {
        return native_ladder_effort(effort);
    }
    let normalized = normalize_anthropic_model(model);
    if normalized.starts_with("claude-opus-4-6") || normalized.starts_with("claude-sonnet-4-6") {
        return four_six_ladder_effort(effort);
    }
    if normalized.starts_with("claude-haiku-5") {
        return haiku_five_ladder_effort(effort);
    }
    // Conservative compatibility for older adaptive models whose effort ladder
    // is not covered by the capability cases above.
    match requested {
        Some(
            ThinkingEffort::Off
            | ThinkingEffort::Minimal
            | ThinkingEffort::Low
            | ThinkingEffort::Medium,
        ) => None,
        Some(ThinkingEffort::High | ThinkingEffort::XHigh | ThinkingEffort::Max) | None => {
            Some("high")
        }
    }
}

fn native_ladder_effort(effort: ThinkingEffort) -> Option<&'static str> {
    Some(match effort {
        ThinkingEffort::Off => return None,
        // Current adaptive Claude 5 models do not expose a separate
        // minimal tier.
        ThinkingEffort::Minimal | ThinkingEffort::Low => "low",
        ThinkingEffort::Medium => "medium",
        ThinkingEffort::High => "high",
        ThinkingEffort::XHigh => "xhigh",
        ThinkingEffort::Max => "max",
    })
}

fn four_six_ladder_effort(effort: ThinkingEffort) -> Option<&'static str> {
    Some(match effort {
        ThinkingEffort::Off => return None,
        ThinkingEffort::Minimal | ThinkingEffort::Low => "low",
        ThinkingEffort::Medium => "medium",
        ThinkingEffort::High | ThinkingEffort::XHigh => "high",
        ThinkingEffort::Max => "max",
    })
}

/// Haiku 5's ladder stops at high (`model_catalog::has_max_effort`).
fn haiku_five_ladder_effort(effort: ThinkingEffort) -> Option<&'static str> {
    Some(match effort {
        ThinkingEffort::Off => return None,
        ThinkingEffort::Minimal | ThinkingEffort::Low => "low",
        ThinkingEffort::Medium => "medium",
        ThinkingEffort::High | ThinkingEffort::XHigh | ThinkingEffort::Max => "high",
    })
}
