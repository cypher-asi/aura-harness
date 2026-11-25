//! Per-iteration logic: LLM calls, response accumulation, and stop-reason handling.

use aura_model::{ContentBlock, ModelProvider, ModelRequest, ModelResponse};
use tokio::sync::mpsc::Sender;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::events::AgentLoopEvent;
use crate::types::AgentLoopResult;
use aura_config::CHARS_PER_TOKEN;

use super::streaming;
use super::{AgentLoop, AgentLoopConfig, LoopState};

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// LLM call error handling
// ---------------------------------------------------------------------------

/// Describes why an LLM call failed, allowing the main loop to break cleanly.
pub(super) enum LlmCallError {
    InsufficientCredits(String),
    /// 401 / 403: the route refused the sign-in or plan behind the request.
    Unauthorized(String),
    PromptTooLong(String),
    /// 429/529 surfaced by the provider. The message already includes the
    /// upstream `retry after N seconds` hint when one was reported so the
    /// UI can show a meaningful wait time. Emitted as `code: "rate_limit"`
    /// so clients can react (e.g. surface a retry affordance) rather than
    /// treat it as a generic fatal LLM error.
    RateLimited(String),
    /// The model could not be reached: upstream 5xx, a failed
    /// connection, a timeout, or a stream that died or went silent.
    Unavailable(String),
    Fatal(String),
}

impl LlmCallError {
    pub(super) fn apply(
        self,
        result: &mut AgentLoopResult,
        event_tx: Option<&Sender<AgentLoopEvent>>,
    ) {
        result.llm_unavailable = matches!(
            self,
            Self::InsufficientCredits(_)
                | Self::Unauthorized(_)
                | Self::RateLimited(_)
                | Self::Unavailable(_)
        );
        match self {
            Self::InsufficientCredits(msg) => {
                result.insufficient_credits = true;
                warn!("Insufficient credits (402), stopping loop");
                streaming::emit(
                    event_tx,
                    AgentLoopEvent::Error {
                        code: crate::events::ERROR_INSUFFICIENT_CREDITS.to_string(),
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
            Self::Unauthorized(msg) => {
                warn!(message = %msg, "Model route refused the credentials, stopping loop");
                streaming::emit(
                    event_tx,
                    AgentLoopEvent::Error {
                        code: crate::events::ERROR_UNAUTHORIZED.to_string(),
                        message: msg.clone(),
                        recoverable: false,
                    },
                );
                result.llm_error = Some(msg);
            }
            Self::PromptTooLong(msg) | Self::Unavailable(msg) | Self::Fatal(msg) => {
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
    /// Convert a structured [`aura_model::ReasonerError`] into an
    /// [`LlmCallError`] with the same credit/context/fatal classification
    /// the loop already applies to non-streaming errors. Kept as a
    /// dedicated constructor so `streaming.rs` can surface errors without
    /// going through `anyhow`.
    pub(super) fn from_reasoner_error(e: &aura_model::ReasonerError) -> Self {
        match e {
            aura_model::ReasonerError::InsufficientCredits(msg) => {
                Self::InsufficientCredits(msg.clone())
            }
            aura_model::ReasonerError::RateLimited { message, .. } => {
                Self::RateLimited(message.clone())
            }
            // Defensive fallback: foreign providers (mock, test
            // harnesses, third-party `ModelProvider` impls) sometimes
            // report rate-limit conditions as `Internal(..)` /
            // `Api { .. }` / `Request(..)` rather than the dedicated
            // `RateLimited` variant. Phase 5 made the kernel gateway
            // preserve the typed `ReasonerError` end-to-end, but we
            // keep this string check so those out-of-band errors still
            // surface as `rate_limit` to SSE consumers.
            other if looks_like_rate_limited(&other.to_string()) => {
                Self::RateLimited(other.to_string())
            }
            other if other.rejects_credentials() => Self::Unauthorized(other.to_string()),
            other if other.is_context_overflow() => Self::PromptTooLong(other.to_string()),
            other if other.is_unavailable() => Self::Unavailable(other.to_string()),
            other => Self::Fatal(other.to_string()),
        }
    }
}

/// Detect a rate-limit error from free-form message text. See
/// [`LlmCallError::from_reasoner_error`] for the rationale — this is a
/// defensive fallback for provider implementations that lose the
/// typed `ReasonerError::RateLimited` variant.
fn looks_like_rate_limited(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("rate limited")
        || lower.contains("rate_limited")
        || lower.contains("too many requests")
}

fn classify_reasoner_error(e: &aura_model::ReasonerError) -> LlmCallError {
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
///
/// Phase 4.6 removed the post-response compaction call that used to
/// live at the tail of this function. Compaction now runs exactly
/// once per sampling turn from [`super::context::compact_if_needed`]
/// at the top of the next iteration. The previous double-call was
/// redundant: every code path that consumed
/// [`compaction::SummaryInput`] from this function also re-entered
/// `compact_if_needed` the very next iteration, so dropping the
/// post-response branch eliminates one full `Compactor::compact_messages`
/// pass per iteration without changing the observable transcript.
///
/// `iteration` is the 0-based index of the model call whose response
/// is being folded in. Kept on the signature for symmetry with
/// [`super::context::effective_compaction_request_kind`] (the next
/// `compact_if_needed` call uses it) and so callers don't have to
/// thread the counter through a different shape per phase.
///
/// Returns `true` when leaked tool-call markup was scrubbed from the
/// assistant text (see [`super::text_sanitize`]). The turn loop uses
/// this to recover the "model wrote a tool call as text, so no tool
/// ran, and the turn ended after one message" failure: the scrubbed
/// leftover text still counts as visible output, so the never-silent
/// no-op nudge does not fire — the caller needs the explicit signal to
/// re-prompt instead of ending the turn.
pub(super) fn accumulate_response(
    _config: &AgentLoopConfig,
    state: &mut LoopState,
    response: &ModelResponse,
    _iteration: usize,
) -> bool {
    state.result.total_input_tokens += response.usage.input_tokens;
    state.result.total_output_tokens += response.usage.output_tokens;
    state.result.total_cache_creation_input_tokens += response
        .usage
        .cache_creation_input_tokens
        .unwrap_or_default();
    state.result.total_cache_read_input_tokens +=
        response.usage.cache_read_input_tokens.unwrap_or_default();

    // Record the *latest* iteration's cache hit/miss counts on the
    // per-turn breakdown. The wire `ContextBreakdown` surfaces this
    // so the UI can render a "Cached this turn" row. If the agent
    // loop runs multiple iterations, the last one wins -- that's the
    // freshest view of "what did the model just see from cache?".
    state.result.context_breakdown.cache_read_tokens =
        response.usage.cache_read_input_tokens.unwrap_or(0);
    state.result.context_breakdown.cache_creation_tokens =
        response.usage.cache_creation_input_tokens.unwrap_or(0);

    // Defensively strip any tool-call markup the model leaked into a
    // text block (Anthropic `<invoke>` / `<function_calls>` XML or the
    // hybrid `[tool_use ... name="...">` shape — see
    // `super::text_sanitize`). Done before the message enters history so
    // the markup is neither re-fed to the model on the next iteration
    // (reinforcing the behaviour) nor carried into the accumulated
    // result text.
    let (sanitized_message, scrubbed_markup) =
        super::text_sanitize::sanitize_message(&response.message);
    if scrubbed_markup {
        warn!("Scrubbed leaked tool-call markup from assistant text before adding to history");
    }

    let mut response_text = String::new();
    for block in &sanitized_message.content {
        match block {
            ContentBlock::Text { text } => {
                append_paragraph(&mut state.result.total_text, text);
                append_paragraph(&mut response_text, text);
            }
            ContentBlock::Thinking { thinking, .. } => {
                state.result.total_thinking.push_str(thinking);
            }
            _ => {}
        }
    }
    if !response_text.is_empty() {
        state.result.final_text = response_text;
    }

    state.messages.push(sanitized_message);

    let raw_message_bytes = state.messages.chars();
    #[allow(clippy::cast_possible_truncation)]
    let message_tokens = (raw_message_bytes / CHARS_PER_TOKEN) as u64;
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

    scrubbed_markup
}

/// Append one complete assistant text block to the turn's accumulated text.
///
/// A turn that alternates text and tool calls produces several text blocks
/// (one or more per model call). Gluing them directly runs sentences
/// together ("...myself.All 3 errors..."), so consecutive blocks are joined
/// with exactly one paragraph break. Whitespace-only blocks are skipped.
///
/// Must be called once per complete block, never per streaming delta, or a
/// single block would be split into paragraphs.
fn append_paragraph(out: &mut String, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    if out.is_empty() {
        out.push_str(text);
        return;
    }
    out.truncate(out.trim_end().len());
    out.push_str("\n\n");
    out.push_str(text.trim_start());
}
