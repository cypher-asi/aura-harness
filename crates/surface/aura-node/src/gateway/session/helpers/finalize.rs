//! Turn finalization: trace recording, usage accounting and the
//! terminal `AssistantMessageEnd` frame.

use super::sink::send_outbound_with_backpressure;
use crate::gateway::session::Session;
use crate::protocol::{
    AssistantMessageEnd, ContextBreakdown, ContextContents, ContextSegment, ErrorMsg, FileDiff,
    FilesChanged, OutboundMessage, SessionUsage,
};
use aura_agent::AgentLoopResult;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};

/// Convert agent-side context segments into their wire equivalents.
fn convert_context_segments(
    segments: &[aura_agent::types::AgentContextSegment],
) -> Vec<ContextSegment> {
    segments
        .iter()
        .map(|seg| ContextSegment {
            label: seg.label.clone(),
            text: seg.text.clone(),
            tokens: seg.tokens,
        })
        .collect()
}

/// Convert the loop's per-turn rendered context contents into the wire
/// [`ContextContents`]. Returns `None` when every bucket is empty so
/// the emitted `SessionUsage` omits the field (matching older-style
/// empty turns) rather than carrying an all-empty payload.
fn build_wire_context_contents(loop_result: &AgentLoopResult) -> Option<ContextContents> {
    let src = &loop_result.context_contents;
    let contents = ContextContents {
        system_prompt: src.system_prompt.clone(),
        tools: convert_context_segments(&src.tools),
        skills: convert_context_segments(&src.skills),
        subagents: convert_context_segments(&src.subagents),
        mcp: convert_context_segments(&src.mcp),
    };
    if contents.is_empty() {
        None
    } else {
        Some(contents)
    }
}

pub(super) fn summarize_files_changed(loop_result: &AgentLoopResult) -> FilesChanged {
    let mut files_changed = FilesChanged::default();
    for change in &loop_result.file_changes {
        match change.kind {
            aura_agent::FileChangeKind::Create => files_changed.created.push(change.path.clone()),
            aura_agent::FileChangeKind::Modify => files_changed.modified.push(change.path.clone()),
            aura_agent::FileChangeKind::Delete => files_changed.deleted.push(change.path.clone()),
        }
        // Emit a per-path diff entry whenever the harness produced
        // non-zero counts. Tools that don't compute a diff (write_file
        // / delete_file today) leave both at 0 and we skip them — the
        // dashboard treats absence of a diff entry as "unknown" rather
        // than "zero lines changed", so omitting beats inserting a
        // misleading 0/0.
        if change.lines_added > 0 || change.lines_removed > 0 {
            files_changed.diffs.push(FileDiff {
                path: change.path.clone(),
                lines_added: change.lines_added,
                lines_removed: change.lines_removed,
            });
        }
    }
    files_changed
}

/// Run trace of `session`, started on first use. `None` unless the
/// operator set `AURA_TRACE`, or when the bundle can't be created.
pub(in crate::gateway::session) fn trace_recorder(
    session: &mut Session,
    model: &str,
) -> Option<Arc<aura_agent_trace::RunRecorder>> {
    if session.trace.is_none() && aura_config::trace().explicit {
        let mut meta = aura_agent_trace::RunMeta::new(model, aura_agent_trace::RunSource::Gateway);
        meta.workspace = session.workspace.display().to_string();
        meta.mode = format!("{:?}", session.mode).to_lowercase();
        meta.task_id = Some(
            session
                .aura_session_id
                .clone()
                .unwrap_or_else(|| session.session_id.clone()),
        );
        session.trace = aura_agent_trace::RunRecorder::start_default(meta);
    }
    session.trace.clone()
}

fn record_turn_end(
    session: &Session,
    join_result: &Result<anyhow::Result<AgentLoopResult>, tokio::task::JoinError>,
) {
    let Some(rec) = &session.trace else {
        return;
    };
    match join_result {
        Ok(Ok(result)) => rec.end_turn(result),
        Ok(Err(e)) => rec.end_turn(&AgentLoopResult {
            llm_error: Some(e.to_string()),
            ..AgentLoopResult::default()
        }),
        Err(e) => rec.end_turn(&AgentLoopResult {
            llm_error: Some(format!("turn task panicked: {e}")),
            ..AgentLoopResult::default()
        }),
    }
    rec.finish();
}

pub(in crate::gateway::session) async fn finalize_turn(
    session: &mut Session,
    join_result: Result<anyhow::Result<AgentLoopResult>, tokio::task::JoinError>,
    message_id: &str,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
) {
    record_turn_end(session, &join_result);
    let result = match join_result {
        Ok(inner) => inner,
        Err(e) => {
            error!(session_id = %session.session_id, error = %e, "Turn task panicked");
            send_turn_error(outbound_tx, message_id).await;
            return;
        }
    };

    match result {
        Ok(loop_result) => {
            apply_turn_result(session, &loop_result, message_id, outbound_tx).await;
        }
        Err(e) => {
            error!(session_id = %session.session_id, error = %e, "Turn processing failed");
            send_outbound_with_backpressure(
                outbound_tx,
                OutboundMessage::Error(ErrorMsg {
                    code: "turn_error".into(),
                    message: format!("Turn processing failed: {e}"),
                    recoverable: true,
                    support_id: None,
                }),
                "turn_error",
            )
            .await;
        }
    }
}

async fn send_turn_error(outbound_tx: &mpsc::Sender<OutboundMessage>, message_id: &str) {
    send_outbound_with_backpressure(
        outbound_tx,
        OutboundMessage::Error(ErrorMsg {
            code: "internal_error".into(),
            message: "Turn processing task panicked".into(),
            recoverable: false,
            support_id: None,
        }),
        "turn_panic_error",
    )
    .await;
    send_outbound_with_backpressure(
        outbound_tx,
        OutboundMessage::AssistantMessageEnd(Box::new(AssistantMessageEnd {
            message_id: message_id.to_string(),
            stop_reason: "error".into(),
            usage: SessionUsage::default(),
            files_changed: FilesChanged::default(),
            originating_user_id: None,
        })),
        "turn_panic_end",
    )
    .await;
}
pub(in crate::gateway::session) async fn apply_turn_result(
    session: &mut Session,
    loop_result: &AgentLoopResult,
    message_id: &str,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
) {
    session.messages.clone_from(&loop_result.messages);
    // Defense-in-depth: cap oversized tool_use inputs / tool_result content
    // so one large blob does not ride along with every subsequent prompt.
    aura_context_compaction::compact_for_storage(&mut session.messages);
    let files_changed = summarize_files_changed(loop_result);

    accumulate_turn_usage(session, loop_result);
    let stop_reason = turn_stop_reason(loop_result);
    let usage = turn_session_usage(session, loop_result);

    send_outbound_with_backpressure(
        outbound_tx,
        OutboundMessage::AssistantMessageEnd(Box::new(AssistantMessageEnd {
            message_id: message_id.to_string(),
            stop_reason: stop_reason.into(),
            usage,
            files_changed,
            originating_user_id: None,
        })),
        "assistant_message_end",
    )
    .await;

    info!(
        session_id = %session.session_id,
        timed_out = loop_result.timed_out,
        iterations = loop_result.iterations,
        history_len = session.messages.len(),
        "Turn complete"
    );
}

fn accumulate_turn_usage(session: &mut Session, loop_result: &AgentLoopResult) {
    session.cumulative_input_tokens += loop_result.total_input_tokens;
    session.cumulative_output_tokens += loop_result.total_output_tokens;
    session.cumulative_cache_creation_input_tokens += loop_result.total_cache_creation_input_tokens;
    session.cumulative_cache_read_input_tokens += loop_result.total_cache_read_input_tokens;
}

fn turn_stop_reason(loop_result: &AgentLoopResult) -> &'static str {
    if loop_result.budget_exhausted {
        "budget"
    } else if loop_result.timed_out {
        "cancelled"
    } else if loop_result.insufficient_credits {
        "insufficient_credits"
    } else if loop_result.llm_error.is_some() {
        "end_turn_with_errors"
    } else if loop_result.output_truncated {
        "max_tokens"
    } else {
        "end_turn"
    }
}

/// Per-turn and cumulative usage; call after [`accumulate_turn_usage`].
fn turn_session_usage(session: &Session, loop_result: &AgentLoopResult) -> SessionUsage {
    let estimated_context_tokens = loop_result.estimated_context_tokens;
    let context_utilization = if session.context_window_tokens > 0 {
        #[allow(clippy::cast_precision_loss)]
        let ratio = estimated_context_tokens as f32 / session.context_window_tokens as f32;
        ratio.min(1.0)
    } else {
        0.0
    };

    SessionUsage {
        input_tokens: loop_result.total_input_tokens,
        output_tokens: loop_result.total_output_tokens,
        estimated_context_tokens,
        cache_creation_input_tokens: loop_result.total_cache_creation_input_tokens,
        cache_read_input_tokens: loop_result.total_cache_read_input_tokens,
        cumulative_input_tokens: session.cumulative_input_tokens,
        cumulative_output_tokens: session.cumulative_output_tokens,
        cumulative_cache_creation_input_tokens: session.cumulative_cache_creation_input_tokens,
        cumulative_cache_read_input_tokens: session.cumulative_cache_read_input_tokens,
        context_utilization,
        model: session.model.clone(),
        provider: session.provider_name.clone(),
        context_breakdown: wire_context_breakdown(loop_result),
        context_contents: build_wire_context_contents(loop_result),
    }
}

fn wire_context_breakdown(loop_result: &AgentLoopResult) -> ContextBreakdown {
    let breakdown = &loop_result.context_breakdown;
    ContextBreakdown {
        system_prompt_tokens: breakdown.system_prompt_tokens,
        tools_tokens: breakdown.tools_tokens,
        skills_tokens: breakdown.skills_tokens,
        mcp_tokens: breakdown.mcp_tokens,
        subagents_tokens: breakdown.subagents_tokens,
        conversation_tokens: breakdown.conversation_tokens,
        cache_read_tokens: breakdown.cache_read_tokens,
        cache_creation_tokens: breakdown.cache_creation_tokens,
    }
}
