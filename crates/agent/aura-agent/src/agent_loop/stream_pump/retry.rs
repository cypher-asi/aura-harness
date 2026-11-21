//! Shared retry classifier and budget for the streaming pump.
//!
//! Phase 1 of the reasoner-retry consolidation seated the canonical
//! retry envelope inside [`aura_config::reasoner()`] so the streaming
//! pump, the legacy buffered streaming retry loop
//! ([`crate::agent_loop::streaming::AgentLoop::stream_retry_params`]),
//! and `aura_model::AnthropicConfig` all read the same
//! `AURA_LLM_MAX_RETRIES` / `AURA_LLM_BACKOFF_INITIAL_MS` /
//! `AURA_LLM_BACKOFF_CAP_MS` triple at startup.
//!
//! Phase 3 collapses the previously-duplicated `stream_retry_params`
//! helper in [`crate::agent_loop::streaming`] onto this one, so
//! operators tune both paths together and a single definition keeps
//! the budget invariant from drifting.

use std::time::Duration;

use aura_model::{PartialToolUse, ThinkingEffort};

/// Retry budget / backoff envelope shared with the legacy buffered
/// streaming retry path and `aura_model::AnthropicConfig`. Both
/// paths now read the same [`aura_config::reasoner().llm_retry`]
/// value (sourced once from `AURA_LLM_MAX_RETRIES` /
/// `AURA_LLM_BACKOFF_INITIAL_MS` / `AURA_LLM_BACKOFF_CAP_MS` at
/// startup) so operators tune both paths together.
pub(in crate::agent_loop) fn stream_retry_params() -> (u32, u64, u64) {
    aura_config::reasoner().llm_retry.as_legacy_triple()
}

/// Captured retry context for one in-flight stream that aborted with
/// a partial `tool_use`. The pump's retry loop seeds this from the
/// most recent `StreamAbortedWithPartial` so subsequent attempts can
/// surface meaningful telemetry (and the final `ToolCallFailed` event
/// names a real tool when the retry budget is exhausted).
pub(super) struct PartialRetryState {
    pub(super) tool_use_id: String,
    pub(super) tool_name: String,
    pub(super) reason: String,
}

/// Fold a fresh `StreamAbortedWithPartial` into the running retry
/// context. The `previous` state's identifiers are kept when the new
/// abort did not carry a [`PartialToolUse`] (i.e. the stream died
/// before `content_block_start` fired), so the eventual
/// `ToolCallFailed` still names the original tool when we have it.
pub(super) fn update_partial_retry_state(
    previous: Option<PartialRetryState>,
    reason: String,
    partial_tool_use: Option<PartialToolUse>,
) -> PartialRetryState {
    let (prev_id, prev_name) = previous.map_or_else(
        || ("<unknown>".to_string(), "<unknown>".to_string()),
        |state| (state.tool_use_id, state.tool_name),
    );
    let (tool_use_id, tool_name) = partial_tool_use.map_or((prev_id, prev_name), |partial| {
        (partial.tool_use_id, partial.tool_name)
    });
    PartialRetryState {
        tool_use_id,
        tool_name,
        reason,
    }
}

/// How long an attempt runs before its interruption counts as the call
/// being too long to finish rather than a dropped connection.
pub(super) const LONG_ATTEMPT: Duration = Duration::from_secs(60);

/// The effort a retry asks for after an attempt was cut off `cut_after`
/// into a call at `effort`: one step lower once the attempt ran
/// [`LONG_ATTEMPT`], since the same reasoning started over is cut off at
/// the same point. `None` keeps the effort.
pub(super) fn effort_after_cut(
    effort: Option<ThinkingEffort>,
    cut_after: Duration,
) -> Option<ThinkingEffort> {
    if cut_after < LONG_ATTEMPT {
        return None;
    }
    match effort? {
        ThinkingEffort::Max | ThinkingEffort::XHigh => Some(ThinkingEffort::High),
        ThinkingEffort::High => Some(ThinkingEffort::Medium),
        ThinkingEffort::Medium => Some(ThinkingEffort::Low),
        _ => None,
    }
}

/// What a restart of the response says: how long the failed attempt ran,
/// what ended it, and the effort it retries at when that was lowered.
pub(super) fn reset_reason(
    cut_after: Duration,
    error: &str,
    lowered: Option<ThinkingEffort>,
) -> String {
    let secs = cut_after.as_secs();
    let ran = if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    };
    let mut reason = format!("Model stream interrupted after {ran}");
    if !error.is_empty() {
        reason.push_str(&format!(" ({error})"));
    }
    reason.push_str("; restarting the response");
    if let Some(effort) = lowered.and_then(ThinkingEffort::reasoning_effort_wire) {
        reason.push_str(&format!(" at {effort} effort"));
    }
    reason
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_cut_off_retries_one_effort_lower_and_a_quick_drop_does_not() {
        let long = Duration::from_secs(121);
        assert_eq!(
            effort_after_cut(Some(ThinkingEffort::Max), long),
            Some(ThinkingEffort::High)
        );
        assert_eq!(
            effort_after_cut(Some(ThinkingEffort::High), long),
            Some(ThinkingEffort::Medium)
        );
        assert_eq!(effort_after_cut(Some(ThinkingEffort::Low), long), None);
        assert_eq!(effort_after_cut(None, long), None);
        assert_eq!(
            effort_after_cut(Some(ThinkingEffort::Max), Duration::from_secs(5)),
            None
        );
    }

    #[test]
    fn the_reset_says_how_long_the_attempt_ran_and_what_ended_it() {
        assert_eq!(
            reset_reason(
                Duration::from_millis(120_400),
                "transport closed before stream completed: missing message_stop",
                Some(ThinkingEffort::Medium),
            ),
            "Model stream interrupted after 2m 00s (transport closed before stream \
             completed: missing message_stop); restarting the response at medium effort"
        );
        assert_eq!(
            reset_reason(Duration::from_secs(3), "", None),
            "Model stream interrupted after 3s; restarting the response"
        );
    }
}
