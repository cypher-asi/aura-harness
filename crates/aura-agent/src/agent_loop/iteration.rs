//! Per-iteration logic: LLM calls, response accumulation, and stop-reason handling.

use aura_reasoner::{
    ContentBlock, Message, ModelProvider, ModelRequest, ModelResponse, ToolResultContent,
};
use tokio::sync::mpsc::Sender;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::compaction;
use crate::constants::{
    CHARS_PER_TOKEN, NARRATION_TOKEN_HARD_BUDGET, NARRATION_TOKEN_SOFT_BUDGET,
    WRITE_FILE_CHUNK_BYTES,
};
use crate::events::AgentLoopEvent;
use crate::helpers;
use crate::sanitize;
use crate::types::AgentLoopResult;

use super::streaming;
use super::{AgentLoop, AgentLoopConfig, LoopState};

// ---------------------------------------------------------------------------
// LLM call error handling
// ---------------------------------------------------------------------------

/// Describes why an LLM call failed, allowing the main loop to break cleanly.
pub(super) enum LlmCallError {
    InsufficientCredits(String),
    PromptTooLong(String),
    /// 429/529 surfaced by the provider. The message already includes the
    /// upstream `retry after N seconds` hint when one was reported so the
    /// UI can show a meaningful wait time. Emitted as `code: "rate_limit"`
    /// so clients can react (e.g. surface a retry affordance) rather than
    /// treat it as a generic fatal LLM error.
    RateLimited(String),
    Fatal(String),
}

impl LlmCallError {
    pub(super) fn apply(
        self,
        result: &mut AgentLoopResult,
        event_tx: Option<&Sender<AgentLoopEvent>>,
    ) {
        match self {
            Self::InsufficientCredits(msg) => {
                result.insufficient_credits = true;
                warn!("Insufficient credits (402), stopping loop");
                streaming::emit(
                    event_tx,
                    AgentLoopEvent::Error {
                        code: "insufficient_credits".to_string(),
                        message: msg,
                        recoverable: false,
                    },
                );
            }
            Self::RateLimited(msg) => {
                warn!(message = %msg, "LLM rate limited after retries, stopping loop");
                streaming::emit(
                    event_tx,
                    AgentLoopEvent::Error {
                        code: "rate_limit".to_string(),
                        message: msg.clone(),
                        // Retries already happened at the provider layer; the
                        // loop cannot recover this turn, but the next user
                        // turn (or a client-side retry) can succeed.
                        recoverable: true,
                    },
                );
                result.llm_error = Some(msg);
            }
            Self::PromptTooLong(msg) | Self::Fatal(msg) => {
                streaming::emit(
                    event_tx,
                    AgentLoopEvent::Error {
                        code: "llm_error".to_string(),
                        message: msg.clone(),
                        recoverable: false,
                    },
                );
                result.llm_error = Some(msg);
            }
        }
    }
}

impl LlmCallError {
    /// Convert a structured [`aura_reasoner::ReasonerError`] into an
    /// [`LlmCallError`] with the same credit/context/fatal classification
    /// the loop already applies to non-streaming errors. Kept as a
    /// dedicated constructor so `streaming.rs` can surface errors without
    /// going through `anyhow`.
    pub(super) fn from_reasoner_error(e: &aura_reasoner::ReasonerError) -> Self {
        match e {
            aura_reasoner::ReasonerError::InsufficientCredits(msg) => {
                Self::InsufficientCredits(msg.clone())
            }
            aura_reasoner::ReasonerError::RateLimited(msg) => Self::RateLimited(msg.clone()),
            // The kernel gateway stringifies errors into `ReasonerError::Internal`
            // (see `kernel_gateway.rs::complete_streaming`), which loses the
            // `RateLimited` variant. Recover the classification from the
            // message text so the SSE error still carries the
            // `rate_limit` code downstream.
            other if looks_like_rate_limited(&other.to_string()) => {
                Self::RateLimited(other.to_string())
            }
            other if other.is_context_overflow() => Self::PromptTooLong(other.to_string()),
            other => Self::Fatal(other.to_string()),
        }
    }
}

/// Detect a rate-limit error from free-form message text, used when a
/// wrapped error has lost its original variant across a crate boundary.
fn looks_like_rate_limited(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("rate limited")
        || lower.contains("rate_limited")
        || lower.contains("too many requests")
}

fn classify_reasoner_error(e: &aura_reasoner::ReasonerError) -> LlmCallError {
    LlmCallError::from_reasoner_error(e)
}

impl AgentLoop {
    /// Call the model and translate errors.
    ///
    /// Uses streaming when `event_tx` is present, non-streaming otherwise.
    pub(super) async fn call_model(
        &self,
        provider: &dyn ModelProvider,
        request: ModelRequest,
        event_tx: Option<&Sender<AgentLoopEvent>>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<ModelResponse, LlmCallError> {
        let stream_timeout = self.config.stream_timeout;

        timeout(stream_timeout, async {
            if event_tx.is_some() {
                self.complete_with_streaming(provider, request, event_tx, cancellation_token)
                    .await
            } else {
                provider
                    .complete(request)
                    .await
                    .map_err(|e| classify_reasoner_error(&e))
            }
        })
        .await
        .unwrap_or_else(|_| {
            Err(LlmCallError::Fatal(format!(
                "Model call timed out after {stream_timeout:?}"
            )))
        })
    }
}

// ---------------------------------------------------------------------------
// Response accumulation
// ---------------------------------------------------------------------------

/// Accumulate token counts, text, and thinking from the model response.
pub(super) fn accumulate_response(state: &mut LoopState, response: &ModelResponse) {
    state.result.total_input_tokens += response.usage.input_tokens;
    state.result.total_output_tokens += response.usage.output_tokens;
    state.result.total_cache_creation_input_tokens += response
        .usage
        .cache_creation_input_tokens
        .unwrap_or_default();
    state.result.total_cache_read_input_tokens +=
        response.usage.cache_read_input_tokens.unwrap_or_default();

    for block in &response.message.content {
        match block {
            ContentBlock::Text { text } => state.result.total_text.push_str(text),
            ContentBlock::Thinking { thinking, .. } => {
                state.result.total_thinking.push_str(thinking);
            }
            _ => {}
        }
    }

    state.messages.push(response.message.clone());
    summarize_write_inputs(&mut state.messages);

    #[allow(clippy::cast_possible_truncation)]
    let message_tokens =
        (compaction::estimate_message_chars(&state.messages) / CHARS_PER_TOKEN) as u64;
    let provider_tokens = response
        .usage
        .input_tokens
        .saturating_add(response.usage.output_tokens)
        .saturating_add(
            response
                .usage
                .cache_creation_input_tokens
                .unwrap_or_default(),
        )
        .saturating_add(response.usage.cache_read_input_tokens.unwrap_or_default());
    let estimated_context_tokens = provider_tokens.max(message_tokens);
    state.last_context_tokens_estimate = Some(estimated_context_tokens);
    state.result.estimated_context_tokens = estimated_context_tokens;
}

/// Replace large write-tool inputs with summaries to save context space.
fn summarize_write_inputs(messages: &mut [Message]) {
    let Some(last_msg) = messages.last_mut() else {
        return;
    };
    for block in &mut last_msg.content {
        if let ContentBlock::ToolUse { name, input, .. } = block {
            if let Some(summarized) = helpers::summarize_write_input(name, input) {
                *input = summarized;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// MaxTokens stop-reason handling
// ---------------------------------------------------------------------------

/// Handle `StopReason::MaxTokens` — inject error results for pending tool calls.
///
/// Returns `true` if the loop should continue, `false` if it should break.
pub(super) fn handle_max_tokens(
    config: &AgentLoopConfig,
    response: &ModelResponse,
    state: &mut LoopState,
) -> bool {
    let pending_tools = extract_pending_tools(response);
    if pending_tools.is_empty() {
        return false;
    }

    warn!(
        pending = pending_tools.len(),
        "MaxTokens with pending tool_use blocks — injecting error results"
    );

    // Signal to `LoopState::begin_iteration` that the next iteration
    // must NOT taper `thinking_budget` — the model is about to retry
    // the dropped tool call(s) and needs the full budget to fit the
    // JSON that just got cut off. Without this reset, a task that
    // hits `max_tokens` mid-edit on iteration N+1 would retry on
    // iteration N+2 with an already-tapered budget and truncate
    // again, producing the observed loop of repeated
    // `MaxTokens with pending tool_use blocks` warnings.
    state.restore_budget_next_iteration = true;

    let results: Vec<(String, ToolResultContent, bool)> = pending_tools
        .iter()
        .map(|pt| {
            let text = synthetic_truncation_message(pt);
            (pt.id.clone(), ToolResultContent::text(text), true)
        })
        .collect();

    state.messages.push(Message::tool_results(results));

    if config.max_context_tokens.is_some() {
        let tier = compaction::CompactionConfig::aggressive();
        compaction::compact_older_messages(&mut state.messages, &tier);
        sanitize::validate_and_repair(&mut state.messages);
    }

    true
}

/// Build the synthetic `tool_result` body injected when a tool call is
/// recovered from a `max_tokens`-truncated stream. Kept as a free
/// function so tests can pin the exact wording the model sees, and so
/// the per-tool branches stay readable.
fn synthetic_truncation_message(pt: &PendingTool) -> String {
    match pt.name.as_str() {
        "write_file" => match pt.path.as_deref() {
            Some(path) => format!(
                "Error: Response was truncated (max_tokens) mid-`write_file`. \
                 Target path: `{path}`. Partial content (if any) is NOT on disk. \
                 Next turn: call `edit_file` on `{path}` with `append_after_eof` to add \
                 remaining content incrementally, or call `write_file` with only the \
                 skeleton (module-doc + imports + one stub) and switch to `edit_file` \
                 appends for the rest."
            ),
            None => "Error: Response was truncated (max_tokens) mid-`write_file` \
                 (no target path recovered). Next turn: retry with the skeleton \
                 (module-doc + imports + one stub) and use `edit_file` \
                 `append_after_eof` for the rest."
                .to_string(),
        },
        "edit_file" => match pt.path.as_deref() {
            Some(path) => format!(
                "Error: Response was truncated (max_tokens) mid-`edit_file`. \
                 Target path: `{path}`. No changes were applied on disk. \
                 Next turn: split the edit into TWO smaller `edit_file` calls \
                 (e.g. change one function or block at a time) rather than one \
                 large diff. Your next `max_tokens` budget is restored to full \
                 for the retry, but each individual tool call should fit in a \
                 few hundred lines of diff."
            ),
            None => "Error: Response was truncated (max_tokens) mid-`edit_file` \
                 (no target path recovered). Next turn: retry with a smaller, \
                 targeted edit scoped to a single function or block."
                .to_string(),
        },
        other => format!(
            "Error: Response was truncated (max_tokens). Tool '{other}' was not executed. \
             Please try again with a simpler approach or break the task into smaller steps."
        ),
    }
}

/// Subset of a pending `tool_use` block used to shape the synthetic
/// error injected on `max_tokens`. `path` is best-effort — extracted
/// from the partial input when it serialized cleanly enough to decode
/// the `path` field before truncation hit.
struct PendingTool {
    id: String,
    name: String,
    path: Option<String>,
}

// ---------------------------------------------------------------------------
// Narration budget (Phase 4 live steering)
// ---------------------------------------------------------------------------

/// Build the steering message text injected at the soft budget. Shared
/// with tests so the assertion and the production string cannot drift.
pub(super) fn narration_steering_message(token_count: usize) -> String {
    format!(
        "[harness steering] The last turns produced {token_count} tokens of text with no tool \
         calls. On your next turn, call exactly ONE tool (read_file, search_code, or write_file \
         \u{2264} {WRITE_FILE_CHUNK_BYTES} bytes). Do NOT narrate a plan."
    )
}

/// Update the per-turn narration counter and, when budgets are crossed,
/// inject a steering user message or stamp a stop-reason override.
///
/// Returns `true` when the loop should break immediately (hard budget
/// exhausted). The caller is expected to invoke this after
/// [`accumulate_response`] and the stop-reason dispatch.
pub(super) fn update_narration_budget(
    event_tx: Option<&Sender<AgentLoopEvent>>,
    state: &mut super::LoopState,
    response: &ModelResponse,
) -> bool {
    let had_tool_call = response
        .message
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse { .. }));

    if had_tool_call {
        state.last_turn_had_tool_call = true;
        state.consecutive_narration_tokens = 0;
        return false;
    }

    state.last_turn_had_tool_call = false;
    let added = usize::try_from(response.usage.output_tokens).unwrap_or(usize::MAX);
    state.consecutive_narration_tokens = state.consecutive_narration_tokens.saturating_add(added);

    // Hard budget takes precedence: we do not want to inject a steering
    // message on a turn we are already aborting.
    if state.consecutive_narration_tokens >= NARRATION_TOKEN_HARD_BUDGET {
        let msg = format!(
            "[harness steering] Narration budget exhausted after {} tokens without a tool call. \
             Stopping the turn so the orchestrator can decompose the task.",
            state.consecutive_narration_tokens
        );
        warn!(
            tokens = state.consecutive_narration_tokens,
            "narration hard budget exhausted, forcing stop_reason_override"
        );
        super::streaming::emit(
            event_tx,
            AgentLoopEvent::Error {
                code: "narration_budget_exhausted".to_string(),
                message: msg,
                recoverable: true,
            },
        );
        state.result.stop_reason_override = Some("narration_budget_exhausted".to_string());
        state.result.stalled = true;
        return true;
    }

    if state.consecutive_narration_tokens >= NARRATION_TOKEN_SOFT_BUDGET {
        let steer = narration_steering_message(state.consecutive_narration_tokens);
        info!(
            tokens = state.consecutive_narration_tokens,
            "narration soft budget crossed, injecting steering user message"
        );
        state.messages.push(Message::user(steer.clone()));
        super::streaming::emit(event_tx, AgentLoopEvent::Warning(steer));
        state.consecutive_narration_tokens = 0;
    }

    false
}

fn extract_pending_tools(response: &ModelResponse) -> Vec<PendingTool> {
    response
        .message
        .content
        .iter()
        .filter_map(|block| {
            if let ContentBlock::ToolUse { id, name, input } = block {
                let path = input
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string);
                Some(PendingTool {
                    id: id.clone(),
                    name: name.clone(),
                    path,
                })
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod rate_limit_tests {
    use super::*;

    #[test]
    fn from_reasoner_error_maps_rate_limited_variant() {
        let err = aura_reasoner::ReasonerError::RateLimited(
            "429 too many requests (retry after 7 seconds)".to_string(),
        );
        match LlmCallError::from_reasoner_error(&err) {
            LlmCallError::RateLimited(msg) => {
                assert!(msg.contains("retry after 7 seconds"), "message: {msg}");
            }
            _ => panic!("expected RateLimited"),
        }
    }

    #[test]
    fn from_reasoner_error_recovers_rate_limited_across_kernel_boundary() {
        // Matches what `KernelModelGateway::complete_streaming` produces
        // when the kernel stringifies a rate-limit error:
        //     ReasonerError::Internal("kernel reason_streaming error: reasoner error: Rate limited: ...")
        let err = aura_reasoner::ReasonerError::Internal(
            "kernel reason_streaming error: reasoner error: Rate limited: \
             Anthropic API error: 429 Too Many Requests - \
             {\"error\":{\"code\":\"RATE_LIMITED\",\"message\":\"Too many requests. Retry after 7 seconds.\"}}"
                .to_string(),
        );
        assert!(
            matches!(
                LlmCallError::from_reasoner_error(&err),
                LlmCallError::RateLimited(_)
            ),
            "expected prose-based rate-limit recovery to kick in"
        );
    }

    #[test]
    fn from_reasoner_error_does_not_confuse_other_errors_with_rate_limited() {
        let err = aura_reasoner::ReasonerError::Api {
            status: 500,
            message: "internal server error".to_string(),
        };
        assert!(matches!(
            LlmCallError::from_reasoner_error(&err),
            LlmCallError::Fatal(_)
        ));
    }

    #[test]
    fn looks_like_rate_limited_is_case_insensitive() {
        assert!(looks_like_rate_limited("Rate Limited: boom"));
        assert!(looks_like_rate_limited("Too Many Requests"));
        assert!(looks_like_rate_limited("code: RATE_LIMITED"));
        assert!(!looks_like_rate_limited("invalid api key"));
    }
}

#[cfg(test)]
mod max_tokens_tests {
    use super::*;
    use aura_reasoner::{ContentBlock, Message, ProviderTrace, Role, Usage};

    use crate::agent_loop::AgentLoopConfig;

    fn tool_use_response(tool_name: &str, path: Option<&str>) -> ModelResponse {
        let input = match path {
            Some(p) => serde_json::json!({"path": p, "content": "stub"}),
            None => serde_json::json!({"content": "stub"}),
        };
        let message = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "toolu_1".into(),
                name: tool_name.into(),
                input,
            }],
        };
        ModelResponse {
            stop_reason: aura_reasoner::StopReason::MaxTokens,
            message,
            usage: Usage::default(),
            trace: ProviderTrace::default(),
            model_used: String::new(),
        }
    }

    fn find_tool_result_text(state: &LoopState) -> Vec<String> {
        let Some(last) = state.messages.last() else {
            return Vec::new();
        };
        last.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult { content, .. } => match content {
                    aura_reasoner::ToolResultContent::Text(t) => Some(t.clone()),
                    aura_reasoner::ToolResultContent::Json(v) => Some(v.to_string()),
                },
                _ => None,
            })
            .collect()
    }

    /// Build a realistic in-progress conversation: a prior user turn
    /// followed by the assistant message with the truncated tool_use
    /// block. handle_max_tokens will push a tool_result Message after
    /// the assistant message, which sanitize::validate_and_repair then
    /// keeps paired correctly.
    fn seed_state_with(config: &AgentLoopConfig, response: &ModelResponse) -> LoopState {
        let initial = vec![Message::user("go write the file"), response.message.clone()];
        LoopState::new(config, initial)
    }

    #[test]
    fn handle_max_tokens_for_write_file_carries_path_hint() {
        let config = AgentLoopConfig::default();
        let response = tool_use_response("write_file", Some("crates/foo/src/lib.rs"));
        let mut state = seed_state_with(&config, &response);

        assert!(handle_max_tokens(&config, &response, &mut state));
        let texts = find_tool_result_text(&state);
        assert_eq!(texts.len(), 1, "one tool_result per pending tool_use");
        let text = &texts[0];
        assert!(
            text.contains("crates/foo/src/lib.rs"),
            "path should appear in the recovery hint, got: {text}"
        );
        assert!(
            text.contains("edit_file") && text.contains("append_after_eof"),
            "recovery pattern should name edit_file + append_after_eof, got: {text}"
        );
    }

    #[test]
    fn handle_max_tokens_for_non_write_tool_uses_generic_text() {
        let config = AgentLoopConfig::default();
        let response = tool_use_response("read_file", Some("src/main.rs"));
        let mut state = seed_state_with(&config, &response);

        assert!(handle_max_tokens(&config, &response, &mut state));
        let texts = find_tool_result_text(&state);
        assert_eq!(texts.len(), 1);
        assert!(
            !texts[0].contains("append_after_eof"),
            "non-write tools should not get the append_after_eof hint"
        );
        assert!(texts[0].contains("truncated"));
    }

    #[test]
    fn handle_max_tokens_for_edit_file_suggests_splitting_the_edit() {
        // Regression: previously `edit_file` fell through to the
        // generic branch ("try a simpler approach"), which gave the
        // model no concrete recovery path. The harness logs showed
        // repeated `edit_file` truncations as a result. The hint
        // must name `edit_file` explicitly and steer toward splitting.
        let config = AgentLoopConfig::default();
        let response = tool_use_response("edit_file", Some("crates/foo/src/lib.rs"));
        let mut state = seed_state_with(&config, &response);

        assert!(handle_max_tokens(&config, &response, &mut state));
        let texts = find_tool_result_text(&state);
        assert_eq!(texts.len(), 1);
        let text = &texts[0];
        assert!(
            text.contains("crates/foo/src/lib.rs"),
            "path must appear in edit_file recovery hint: {text}"
        );
        assert!(
            text.to_ascii_lowercase().contains("split")
                || text.to_ascii_lowercase().contains("smaller"),
            "edit_file hint should steer toward splitting the edit: {text}"
        );
    }

    #[test]
    fn handle_max_tokens_sets_budget_restore_flag() {
        // The flag is the contract between `handle_max_tokens` and
        // `LoopState::begin_iteration`: truncation implies "next turn
        // needs full budget". Without this, a tapered budget carries
        // into the retry and the model hits `max_tokens` again.
        let config = AgentLoopConfig::default();
        let response = tool_use_response("edit_file", Some("src/x.rs"));
        let mut state = seed_state_with(&config, &response);
        assert!(!state.restore_budget_next_iteration, "precondition");

        assert!(handle_max_tokens(&config, &response, &mut state));
        assert!(
            state.restore_budget_next_iteration,
            "handle_max_tokens must arm the budget-restore flag"
        );
    }

    #[test]
    fn begin_iteration_restores_budget_and_clears_flag() {
        // Given a tapered budget and the restore flag set,
        // begin_iteration must lift the budget back to `max_tokens`
        // and clear the flag so the *next* iteration can taper again.
        let config = AgentLoopConfig::default();
        let mut state = LoopState::new(&config, vec![Message::user("go")]);
        state.thinking_budget = 512;
        state.restore_budget_next_iteration = true;

        // Iteration number is irrelevant for the restore path — the
        // flag short-circuits before the taper branch.
        state.begin_iteration(&config, 99);

        assert_eq!(
            state.thinking_budget, config.max_tokens,
            "budget must be restored to max_tokens after truncation"
        );
        assert!(
            !state.restore_budget_next_iteration,
            "flag must be cleared after a single restore"
        );
    }

    #[test]
    fn begin_iteration_respects_min_budget_floor() {
        // Even after a long run with aggressive tapering, the budget
        // must never fall below `thinking_min_budget`. The floor is
        // what keeps a multi-KB tool-call JSON serializable.
        let config = AgentLoopConfig {
            thinking_taper_after: 0,
            thinking_taper_factor: 0.1,
            ..AgentLoopConfig::default()
        };
        let mut state = LoopState::new(&config, vec![Message::user("go")]);

        for i in 0..50 {
            state.begin_iteration(&config, i);
            assert!(
                state.thinking_budget >= config.thinking_min_budget,
                "budget dropped below floor at iteration {i}: {}",
                state.thinking_budget
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Narration budget tests (Phase 4)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod narration_budget_tests {
    use super::*;
    use aura_reasoner::{ContentBlock, Message, ProviderTrace, Role, Usage};

    use crate::agent_loop::{AgentLoopConfig, LoopState};

    fn text_only_response(output_tokens: u64) -> ModelResponse {
        let message = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "planning and narrating without a tool call".into(),
            }],
        };
        ModelResponse {
            stop_reason: aura_reasoner::StopReason::EndTurn,
            message,
            usage: Usage {
                output_tokens,
                ..Usage::default()
            },
            trace: ProviderTrace::default(),
            model_used: String::new(),
        }
    }

    fn tool_use_response(output_tokens: u64) -> ModelResponse {
        let message = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "brief preamble".into(),
                },
                ContentBlock::ToolUse {
                    id: "toolu_narr".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({"path": "src/lib.rs"}),
                },
            ],
        };
        ModelResponse {
            stop_reason: aura_reasoner::StopReason::ToolUse,
            message,
            usage: Usage {
                output_tokens,
                ..Usage::default()
            },
            trace: ProviderTrace::default(),
            model_used: String::new(),
        }
    }

    fn fresh_state() -> LoopState {
        let config = AgentLoopConfig::default();
        LoopState::new(&config, vec![Message::user("do the task")])
    }

    fn last_user_text(state: &LoopState) -> Option<String> {
        state.messages.iter().rev().find_map(|m| {
            if matches!(m.role, Role::User) {
                m.content.iter().find_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
            } else {
                None
            }
        })
    }

    #[test]
    fn narration_counter_resets_on_tool_use() {
        let mut state = fresh_state();
        state.consecutive_narration_tokens = 900;
        state.last_turn_had_tool_call = false;

        let response = tool_use_response(300);
        assert!(!update_narration_budget(None, &mut state, &response));
        assert_eq!(state.consecutive_narration_tokens, 0);
        assert!(state.last_turn_had_tool_call);
        assert!(state.result.stop_reason_override.is_none());
    }

    #[test]
    fn narration_counter_accumulates_across_toolfree_turns() {
        let mut state = fresh_state();

        let first = text_only_response(400);
        assert!(!update_narration_budget(None, &mut state, &first));
        assert_eq!(state.consecutive_narration_tokens, 400);
        assert!(!state.last_turn_had_tool_call);

        let second = text_only_response(500);
        assert!(!update_narration_budget(None, &mut state, &second));
        assert_eq!(state.consecutive_narration_tokens, 900);
        assert!(!state.last_turn_had_tool_call);
        assert!(state.result.stop_reason_override.is_none());
    }

    #[test]
    fn soft_budget_injects_steering_message() {
        let mut state = fresh_state();
        let messages_before = state.messages.len();

        let big = text_only_response(NARRATION_TOKEN_SOFT_BUDGET as u64);
        assert!(!update_narration_budget(None, &mut state, &big));

        assert_eq!(
            state.consecutive_narration_tokens, 0,
            "soft budget should reset the counter after injection"
        );
        assert_eq!(
            state.messages.len(),
            messages_before + 1,
            "exactly one steering user message should be appended"
        );

        let injected = last_user_text(&state).expect("steering user text");
        assert!(
            injected.contains("harness steering"),
            "should carry the [harness steering] prefix, got: {injected}"
        );
        assert!(
            injected.contains("call exactly ONE tool"),
            "should tell the model to call exactly one tool, got: {injected}"
        );
        assert!(
            injected.contains("12000 bytes"),
            "should cite the Phase 1 write chunk cap, got: {injected}"
        );
        assert!(
            state.result.stop_reason_override.is_none(),
            "soft budget alone should not set a stop reason"
        );
    }

    #[test]
    fn hard_budget_terminates_or_signals() {
        let mut state = fresh_state();

        let exhaust = text_only_response(NARRATION_TOKEN_HARD_BUDGET as u64);
        let should_break = update_narration_budget(None, &mut state, &exhaust);

        assert!(
            should_break,
            "hard budget must signal the loop to break immediately"
        );
        assert_eq!(
            state.result.stop_reason_override.as_deref(),
            Some("narration_budget_exhausted"),
            "stop_reason_override must carry the narration_budget_exhausted code"
        );
        assert!(
            state.result.stalled,
            "hard budget should mark the result as stalled for downstream observability"
        );
    }
}
