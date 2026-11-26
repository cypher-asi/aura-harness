//! Context pressure and compaction-tier selection.

use aura_model::ModelRequestKind;

use super::config::{CompactionConfig, CompactionInput, CompactionPolicy};
use super::size::estimate_message_chars;

pub(super) const DEV_LOOP_BOOTSTRAP_TOTAL_TEXT_MAX_BYTES: usize = 24 * 1024;
const PROJECT_TOOL_TOTAL_TEXT_MAX_BYTES: usize = 48 * 1024;

/// Select the compaction tier based on context utilization percentage.
///
/// Higher utilization means more aggressive compaction. Returns `None` below 15%.
#[must_use]
pub fn select_tier(utilization: f64) -> Option<CompactionConfig> {
    tier_for(utilization)
}

pub(super) fn tier_for(pressure: f64) -> Option<CompactionConfig> {
    let tiers = aura_config::agent().compaction;
    if pressure >= tiers.tier_history {
        Some(CompactionConfig::emergency())
    } else if pressure >= tiers.tier_aggressive {
        Some(CompactionConfig::aggressive())
    } else if pressure >= tiers.tier_60 {
        Some(CompactionConfig::moderate())
    } else if pressure >= tiers.tier_30 {
        Some(CompactionConfig::light())
    } else if pressure >= tiers.tier_micro {
        Some(CompactionConfig::minimal())
    } else {
        None
    }
}

/// Compute the effective pressure used for tier selection.
///
/// Pressure is the maximum of two real signals:
///
/// * **Context pressure** — `current_context_tokens / max_context_tokens`.
///   This is the only honest measure of how full the model's context
///   window is.
/// * **Request-body-cap pressure** — `raw_message_bytes / request_body_cap`,
///   used only on request kinds that actually have an upstream body
///   cap (`DevLoopBootstrap`, `ProjectToolSpecGen`, etc.). Plain `Chat`
///   continuations have no cap so this term is zero.
///
/// We deliberately do *not* drive pressure off raw message-byte count
/// alone — every modern model we target has a context window measured
/// in hundreds of thousands of tokens, and synthetic byte-envelope
/// pressure was silently rewriting tool results long before real
/// pressure existed.
#[must_use]
pub fn effective_pressure(input: &CompactionInput<'_>) -> f64 {
    let policy = input.policy;
    let context_pressure = policy.max_context_tokens.map_or(0.0, |max_ctx| {
        if max_ctx == 0 {
            return 0.0;
        }
        let context_tokens = policy
            .current_context_tokens
            .unwrap_or(policy.estimated_context_tokens)
            .max(policy.estimated_context_tokens);
        let pressure_tokens = context_tokens
            .saturating_add(policy.reserved_output_tokens)
            .min(max_ctx);
        #[allow(clippy::cast_precision_loss)]
        {
            pressure_tokens as f64 / max_ctx as f64
        }
    });
    let request_cap_pressure = request_body_cap(policy).map_or(0.0, |cap| {
        let raw_bytes = policy
            .raw_message_bytes
            .unwrap_or_else(|| estimate_message_chars(input.messages));
        cap_pressure(raw_bytes, cap)
    });

    context_pressure.max(request_cap_pressure).min(1.0)
}

pub(super) fn request_body_cap(policy: CompactionPolicy) -> Option<usize> {
    policy
        .request_body_cap_bytes
        .or_else(|| match policy.request_kind? {
            ModelRequestKind::DevLoopBootstrap => Some(DEV_LOOP_BOOTSTRAP_TOTAL_TEXT_MAX_BYTES),
            ModelRequestKind::ProjectToolSpecGen | ModelRequestKind::ProjectToolTaskExtract => {
                Some(PROJECT_TOOL_TOTAL_TEXT_MAX_BYTES)
            }
            ModelRequestKind::Chat
            | ModelRequestKind::DevLoopContinuation
            | ModelRequestKind::Auxiliary => None,
        })
}

fn cap_pressure(bytes: usize, cap: usize) -> f64 {
    if cap == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    {
        bytes as f64 / cap as f64
    }
}

/// Pick whichever tier trims more aggressively.
#[must_use]
pub fn pick_stricter_tier(
    a: Option<CompactionConfig>,
    b: Option<CompactionConfig>,
) -> Option<CompactionConfig> {
    match (a, b) {
        (Some(x), Some(y)) => {
            if y.tool_result_max_chars < x.tool_result_max_chars {
                Some(y)
            } else {
                Some(x)
            }
        }
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}
