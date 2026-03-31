//! Helper functions for WebSocket session management: init, executor
//! construction, event forwarding, and turn finalization.

use super::ws_handler::populate_tool_definitions;
use super::{Session, WsContext};
use crate::protocol::{
    self, AssistantMessageEnd, ErrorMsg, FilesChanged, OutboundMessage, SessionInit, SessionReady,
    SessionUsage, TextDelta, ThinkingDelta, ToolInfo, ToolResultMsg, ToolUseStart,
};
use aura_agent::{AgentLoopEvent, AgentLoopResult, KernelToolExecutor};
use aura_kernel::ExecutorRouter;
use aura_tools::ToolResolver;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};

pub(super) fn handle_session_init(
    session: &mut Session,
    init: SessionInit,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    ctx: &WsContext,
) {
    if session.initialized {
        let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
            code: "already_initialized".into(),
            message: "Session has already been initialized".into(),
            recoverable: true,
        }));
        return;
    }

    if let Err(e) = session.apply_init(init) {
        let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
            code: "invalid_workspace".into(),
            message: e,
            recoverable: true,
        }));
        return;
    }

    if let (Some(ref base), Some(ref pp)) = (&ctx.project_base, &session.project_path) {
        let slug = pp.file_name().and_then(|n| n.to_str()).unwrap_or("default");
        session.project_path = Some(base.join(slug));
    }

    populate_tool_definitions(session, ctx);

    let tools: Vec<ToolInfo> = session
        .tool_definitions
        .iter()
        .map(protocol::tool_info_from_definition)
        .collect();

    info!(
        session_id = %session.session_id,
        model = %session.model,
        tool_count = tools.len(),
        "Session initialized"
    );

    let _ = outbound_tx.try_send(OutboundMessage::SessionReady(SessionReady {
        session_id: session.session_id.clone(),
        tools,
    }));
}

pub(super) fn build_kernel_executor(
    session: &Session,
    ctx: &WsContext,
) -> KernelToolExecutor {
    let mut resolver = ToolResolver::new(ctx.catalog.clone(), ctx.tool_config.clone());

    if let Some(ref domain_api) = ctx.domain_api {
        use aura_tools::domain_tools::DomainToolExecutor;
        let domain_exec = Arc::new(DomainToolExecutor::with_session_context(
            domain_api.clone(),
            session.auth_token.clone(),
            session.project_id.clone(),
        ));
        resolver = resolver.with_domain_executor(domain_exec);
    }

    if let Some(ref controller) = ctx.automaton_controller {
        let project_id = session.project_id.clone().unwrap_or_default();
        let workspace_root = session.project_path.clone();
        for tool in aura_tools::automaton_tools::devloop_control_tools(
            controller.clone(),
            project_id,
            workspace_root,
            session.auth_token.clone(),
        ) {
            resolver.register(tool);
        }
    }

    let mut executor_router = ExecutorRouter::new();
    executor_router.add_executor(Arc::new(resolver));

    let workspace = match session.project_path {
        Some(ref pp) => pp.clone(),
        None => session.workspace.join(session.agent_id.to_hex()),
    };
    KernelToolExecutor::new(executor_router, session.agent_id, workspace)
}

pub(super) async fn forward_events_to_ws(
    mut event_rx: mpsc::Receiver<AgentLoopEvent>,
    outbound: mpsc::Sender<OutboundMessage>,
) {
    while let Some(event) = event_rx.recv().await {
        let msg = match event {
            AgentLoopEvent::TextDelta(text) => OutboundMessage::TextDelta(TextDelta { text }),
            AgentLoopEvent::ThinkingDelta(thinking) => {
                OutboundMessage::ThinkingDelta(ThinkingDelta { thinking })
            }
            AgentLoopEvent::ToolStart { id, name } => {
                OutboundMessage::ToolUseStart(ToolUseStart { id, name })
            }
            AgentLoopEvent::ToolResult {
                tool_use_id,
                tool_name,
                content,
                is_error,
            } => OutboundMessage::ToolResult(ToolResultMsg {
                name: tool_name,
                result: content,
                is_error,
                tool_use_id: Some(tool_use_id),
            }),
            AgentLoopEvent::Error {
                code,
                message,
                recoverable,
            } => OutboundMessage::Error(ErrorMsg {
                code,
                message,
                recoverable,
            }),
            AgentLoopEvent::ToolInputSnapshot { .. }
            | AgentLoopEvent::ToolComplete { .. }
            | AgentLoopEvent::IterationComplete { .. }
            | AgentLoopEvent::ThinkingComplete
            | AgentLoopEvent::StepComplete
            | AgentLoopEvent::Warning(_) => continue,
        };
        if outbound.try_send(msg).is_err() {
            break;
        }
    }
}

pub(super) fn finalize_turn(
    session: &mut Session,
    join_result: Result<anyhow::Result<AgentLoopResult>, tokio::task::JoinError>,
    message_id: &str,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
) {
    let result = match join_result {
        Ok(inner) => inner,
        Err(e) => {
            error!(session_id = %session.session_id, error = %e, "Turn task panicked");
            send_turn_error(outbound_tx, message_id);
            return;
        }
    };

    match result {
        Ok(loop_result) => {
            apply_turn_result(session, &loop_result, message_id, outbound_tx);
        }
        Err(e) => {
            error!(session_id = %session.session_id, error = %e, "Turn processing failed");
            let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
                code: "turn_error".into(),
                message: format!("Turn processing failed: {e}"),
                recoverable: true,
            }));
        }
    }
}

fn send_turn_error(outbound_tx: &mpsc::Sender<OutboundMessage>, message_id: &str) {
    let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
        code: "internal_error".into(),
        message: "Turn processing task panicked".into(),
        recoverable: false,
    }));
    let _ = outbound_tx.try_send(OutboundMessage::AssistantMessageEnd(AssistantMessageEnd {
        message_id: message_id.to_string(),
        stop_reason: "error".into(),
        usage: SessionUsage::default(),
        files_changed: FilesChanged::default(),
    }));
}

pub(super) fn apply_turn_result(
    session: &mut Session,
    loop_result: &AgentLoopResult,
    message_id: &str,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
) {
    session.messages.clone_from(&loop_result.messages);

    let input_tokens = loop_result.total_input_tokens;
    let output_tokens = loop_result.total_output_tokens;
    session.cumulative_input_tokens += input_tokens;
    session.cumulative_output_tokens += output_tokens;

    let stop_reason = if loop_result.timed_out {
        "cancelled"
    } else if loop_result.insufficient_credits {
        "insufficient_credits"
    } else if loop_result.llm_error.is_some() {
        "end_turn_with_errors"
    } else {
        "end_turn"
    };

    let context_utilization = if session.context_window_tokens > 0 {
        #[allow(clippy::cast_precision_loss)]
        let ratio = input_tokens as f32 / session.context_window_tokens as f32;
        ratio.min(1.0)
    } else {
        0.0
    };

    let _ = outbound_tx.try_send(OutboundMessage::AssistantMessageEnd(AssistantMessageEnd {
        message_id: message_id.to_string(),
        stop_reason: stop_reason.into(),
        usage: SessionUsage {
            input_tokens,
            output_tokens,
            cumulative_input_tokens: session.cumulative_input_tokens,
            cumulative_output_tokens: session.cumulative_output_tokens,
            context_utilization,
            model: session.model.clone(),
            provider: String::new(),
        },
        files_changed: FilesChanged::default(),
    }));

    info!(
        session_id = %session.session_id,
        timed_out = loop_result.timed_out,
        iterations = loop_result.iterations,
        history_len = session.messages.len(),
        "Turn complete"
    );
}
