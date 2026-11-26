//! Stop-reason dispatch and the pre-dispatch exploration block.

use aura_model::{Message, ModelResponse};

use crate::events::AgentLoopEvent;
use crate::helpers;
use crate::types::{ToolCallInfo, ToolCallResult};

use super::super::event_sink::emit as emit_event;
use super::super::{AgentLoop, LoopState};
use super::{process_tool_results, tool_calls, ToolBatch, ToolEffectCtx};

/// What a response that ran out of room before it made a tool call is
/// told: nothing in it was carried out, and the next one reasons less,
/// which it is to spend acting.
pub(crate) const CUT_OFF_BEFORE_ACTING: &str = "Your response reached the output-token limit \
     before it made a tool call, so none of it was carried out. The next response reasons at a \
     lower effort. Don't plan again what you already decided: make the next tool call now. \
     Write a large file in parts: create it with the first part, then add the rest with \
     further edits.";

/// Unified stop-reason dispatch.
///
/// Phase 4: collapses the buffered-path `AgentLoop::dispatch_stop_reason`
/// and the pump-path `stream_pump::dispatch::dispatch_streamed_response`
/// into a single function fed by [`ToolBatch`]. Returns `true` when
/// the sampling loop should break.
pub(crate) async fn dispatch(
    agent: &AgentLoop,
    state: &mut LoopState,
    response: &ModelResponse,
    batch: ToolBatch,
    ctx: ToolEffectCtx<'_>,
) -> bool {
    use aura_model::StopReason;
    state.result.output_truncated = response.stop_reason == StopReason::MaxTokens;
    if !state.result.output_truncated {
        state.output_limit_recoveries = 0;
    }
    match response.stop_reason {
        StopReason::EndTurn | StopReason::StopSequence => true,
        StopReason::MaxTokens => dispatch_max_tokens(agent, state, response, batch, ctx).await,
        StopReason::ToolUse => {
            process_tool_results(state, agent, batch, ctx)
                .await
                .should_stop
        }
    }
}

async fn dispatch_max_tokens(
    agent: &AgentLoop,
    state: &mut LoopState,
    response: &ModelResponse,
    batch: ToolBatch,
    ctx: ToolEffectCtx<'_>,
) -> bool {
    let event_tx = ctx.event_tx;
    // The pump has already executed complete calls. Commit their real
    // results before continuing; never replace successful writes with
    // synthetic failures or replay the entire sample.
    let acted = !tool_calls(response).is_empty();
    if acted
        && process_tool_results(state, agent, batch, ctx)
            .await
            .should_stop
    {
        return true;
    }
    const MAX_OUTPUT_LIMIT_RECOVERIES: usize = 2;
    if state.output_limit_recoveries >= MAX_OUTPUT_LIMIT_RECOVERIES {
        emit_event(event_tx, AgentLoopEvent::Error {
            code: "output_limit".into(),
            message: "The response is incomplete: output-token limit reached after bounded continuation. Completed tool actions were preserved.".into(),
            recoverable: false,
        });
        return true;
    }
    state.output_limit_recoveries += 1;
    if acted {
        state.messages.push(Message::user(
            "Your response reached the output-token limit. Continue from the interruption without repeating prior text or completed tools. Use smaller complete tool calls if needed; finish the requested work and clearly report any remaining limitation."
        ));
    } else {
        state.reason_less();
        state.messages.push(Message::user(CUT_OFF_BEFORE_ACTING));
    }
    false
}

/// Pre-dispatch exploration block (on by default).
///
/// When `aura_config::agent().steering.implement_now_hard_block` is `true`
/// (set by `AURA_AGENT_IMPLEMENT_NOW_BLOCK`) and the loop has injected the
/// one-shot `implement_now` steering without any cumulative file writes,
/// further read/search tool calls are short-circuited with a synthetic error
/// result.
pub(in crate::agent_loop) fn partition_circling_duplicate_reads(
    tool_calls: &[ToolCallInfo],
    state: &LoopState,
) -> (Vec<ToolCallResult>, Vec<ToolCallInfo>) {
    let block_enabled = aura_config::agent().steering.implement_now_hard_block;
    if !block_enabled || !state.implement_now_injected || state.had_any_file_write {
        return (Vec::new(), tool_calls.to_vec());
    }

    let mut blocked = Vec::new();
    let mut remaining = Vec::with_capacity(tool_calls.len());
    for tool in tool_calls {
        if helpers::is_exploration_tool(&tool.name) {
            blocked.push(ToolCallResult {
                tool_use_id: tool.id.clone(),
                content: aura_context_prompts::model_messages::implement_now::IMPLEMENT_NOW_HARD_BLOCK_BODY
                    .to_string(),
                is_error: true,
                kind: aura_core_types::ToolResultKind::AgentError,
                stop_loop: false,
                file_changes: Vec::new(),
                image: None,
            });
        } else {
            remaining.push(tool.clone());
        }
    }
    (blocked, remaining)
}
