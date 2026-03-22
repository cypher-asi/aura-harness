//! Per-iteration logic: LLM calls, response accumulation, and stop-reason handling.

use aura_reasoner::{
    ContentBlock, Message, ModelProvider, ModelRequest, ModelResponse, ToolResultContent,
};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::compaction;
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
    Fatal(String),
}

impl LlmCallError {
    pub(super) fn apply(
        self,
        result: &mut AgentLoopResult,
        event_tx: &Option<UnboundedSender<AgentLoopEvent>>,
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
            Self::Fatal(msg) => {
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

fn classify_error(e: anyhow::Error) -> LlmCallError {
    let msg = e.to_string();
    if msg.contains("402") {
        LlmCallError::InsufficientCredits(msg)
    } else {
        LlmCallError::Fatal(msg)
    }
}

impl AgentLoop {
    /// Call the model (streaming or non-streaming) and translate errors.
    pub(super) async fn call_model(
        &self,
        provider: &dyn ModelProvider,
        request: ModelRequest,
        event_tx: &Option<UnboundedSender<AgentLoopEvent>>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<ModelResponse, LlmCallError> {
        if event_tx.is_some() {
            self.complete_with_streaming(provider, request, event_tx, cancellation_token)
                .await
                .map_err(classify_error)
        } else {
            provider.complete(request).await.map_err(classify_error)
        }
    }
}

// ---------------------------------------------------------------------------
// Response accumulation
// ---------------------------------------------------------------------------

/// Accumulate token counts, text, and thinking from the model response.
pub(super) fn accumulate_response(state: &mut LoopState, response: &ModelResponse) {
    state.result.total_input_tokens += response.usage.input_tokens;
    state.result.total_output_tokens += response.usage.output_tokens;
    state.last_input_tokens = Some(response.usage.input_tokens);

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

    let results: Vec<(String, ToolResultContent, bool)> = pending_tools
        .iter()
        .map(|(id, name)| {
            (
                id.clone(),
                ToolResultContent::text(format!(
                    "Error: Response was truncated (max_tokens). Tool '{name}' was not executed. \
                     Please try again with a simpler approach or break the task into smaller steps."
                )),
                true,
            )
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

fn extract_pending_tools(response: &ModelResponse) -> Vec<(String, String)> {
    response
        .message
        .content
        .iter()
        .filter_map(|block| {
            if let ContentBlock::ToolUse { id, name, .. } = block {
                Some((id.clone(), name.clone()))
            } else {
                None
            }
        })
        .collect()
}
