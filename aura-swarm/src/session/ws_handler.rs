//! WebSocket connection handler and turn management.

use super::{Session, WsContext};
use crate::protocol::{
    AssistantMessageEnd, AssistantMessageStart, ErrorMsg, FilesChanged, InboundMessage,
    OutboundMessage, SessionInit, SessionReady, SessionUsage, TextDelta, ToolInfo, UserMessage,
};
use aura_agent::{AgentLoop, AgentLoopResult, KernelToolExecutor};
use aura_executor::ExecutorRouter;
use aura_reasoner::Message;
use aura_tools::{DefaultToolRegistry, ToolExecutor, ToolRegistry};
use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};
use uuid::Uuid;

/// State for a turn that is currently being processed in the background.
struct ActiveTurn {
    /// Token to signal cancellation of the turn.
    cancel_token: CancellationToken,
    /// Handle to the spawned turn-processing task.
    join_handle: JoinHandle<anyhow::Result<AgentLoopResult>>,
    /// Handle to the (no-op) stream-forwarding task.
    stream_forward_handle: JoinHandle<()>,
    /// Message ID for this turn (used in `assistant_message_end`).
    message_id: String,
}

/// Classification of a raw WebSocket frame.
enum WsAction {
    /// A text message was received.
    Message(String),
    /// The connection should be closed.
    Close,
    /// Non-actionable frame (ping/pong/binary); continue the loop.
    Continue,
}

/// Classify a raw WebSocket receive result.
fn classify_ws_frame(msg_result: Option<Result<WsMessage, axum::Error>>) -> WsAction {
    match msg_result {
        Some(Ok(WsMessage::Text(text))) => WsAction::Message(text),
        Some(Ok(WsMessage::Close(_)) | Err(_)) | None => WsAction::Close,
        Some(Ok(_)) => WsAction::Continue,
    }
}

/// Handle a WebSocket connection through its full lifecycle.
///
/// Protocol:
/// 1. Client sends `session_init` as the first message.
/// 2. Server responds with `session_ready`.
/// 3. Client sends `user_message` events, server streams responses.
/// 4. Message history accumulates across turns for multi-turn conversation.
/// 5. Client can send `cancel` during a turn to abort it.
pub async fn handle_ws_connection(socket: WebSocket, ctx: WsContext) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<OutboundMessage>();

    let send_task = tokio::spawn(async move {
        while let Some(msg) = outbound_rx.recv().await {
            match serde_json::to_string(&msg) {
                Ok(json) => {
                    if ws_tx.send(WsMessage::Text(json)).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    error!(error = %e, "Failed to serialize outbound message");
                }
            }
        }
    });

    let mut session = Session::new(ctx.workspace_base.clone());
    session.auth_token = ctx.auth_token.clone();
    info!(session_id = %session.session_id, "WebSocket connection opened");

    let mut active_turn: Option<ActiveTurn> = None;

    loop {
        if let Some(ref mut turn) = active_turn {
            tokio::select! {
                biased;

                msg_result = ws_rx.next() => {
                    match classify_ws_frame(msg_result) {
                        WsAction::Message(raw) => {
                            match serde_json::from_str::<InboundMessage>(&raw) {
                                Ok(InboundMessage::Cancel) => {
                                    info!(session_id = %session.session_id, "Cancelling active turn");
                                    turn.cancel_token.cancel();
                                }
                                Ok(_) => {
                                    let _ = outbound_tx.send(OutboundMessage::Error(ErrorMsg {
                                        code: "turn_in_progress".into(),
                                        message: "A turn is currently in progress; send cancel first".into(),
                                        recoverable: true,
                                    }));
                                }
                                Err(e) => {
                                    let _ = outbound_tx.send(OutboundMessage::Error(ErrorMsg {
                                        code: "parse_error".into(),
                                        message: format!("Invalid message: {e}"),
                                        recoverable: true,
                                    }));
                                }
                            }
                        }
                        WsAction::Close => {
                            debug!(session_id = %session.session_id, "Client closed during active turn");
                            turn.cancel_token.cancel();
                            break;
                        }
                        WsAction::Continue => {}
                    }
                }

                join_result = &mut turn.join_handle => {
                    let finished = active_turn.take().expect("active_turn was Some");
                    let _ = finished.stream_forward_handle.await;
                    finalize_turn(&mut session, join_result, &finished.message_id, &outbound_tx);
                }
            }
        } else {
            match classify_ws_frame(ws_rx.next().await) {
                WsAction::Message(raw) => match serde_json::from_str::<InboundMessage>(&raw) {
                    Ok(InboundMessage::SessionInit(init)) => {
                        handle_session_init(&mut session, init, &outbound_tx);
                    }
                    Ok(InboundMessage::UserMessage(msg)) => {
                        match start_turn(&mut session, msg, &outbound_tx, &ctx) {
                            Some(turn) => active_turn = Some(turn),
                            None => {}
                        }
                    }
                    Ok(InboundMessage::Cancel) => {
                        debug!(session_id = %session.session_id, "Cancel received but no turn is active");
                    }
                    Ok(InboundMessage::ApprovalResponse(resp)) => {
                        debug!(
                            session_id = %session.session_id,
                            tool_use_id = %resp.tool_use_id,
                            approved = resp.approved,
                            "Approval response received (not yet implemented)"
                        );
                    }
                    Err(e) => {
                        let _ = outbound_tx.send(OutboundMessage::Error(ErrorMsg {
                            code: "parse_error".into(),
                            message: format!("Invalid message: {e}"),
                            recoverable: true,
                        }));
                    }
                },
                WsAction::Close => {
                    debug!(session_id = %session.session_id, "Client sent close frame");
                    break;
                }
                WsAction::Continue => {}
            }
        }
    }

    info!(session_id = %session.session_id, "WebSocket connection closed");
    drop(outbound_tx);
    let _ = send_task.await;
}

/// Handle a `session_init` message.
fn handle_session_init(
    session: &mut Session,
    init: SessionInit,
    outbound_tx: &mpsc::UnboundedSender<OutboundMessage>,
) {
    if session.initialized {
        let _ = outbound_tx.send(OutboundMessage::Error(ErrorMsg {
            code: "already_initialized".into(),
            message: "Session has already been initialized".into(),
            recoverable: true,
        }));
        return;
    }

    session.apply_init(init);

    let builtin_tools = DefaultToolRegistry::new();
    session.tool_definitions = builtin_tools.list();

    for ext in &session.external_tools {
        session.tool_definitions.push(aura_reasoner::ToolDefinition::new(
            &ext.name,
            &ext.description,
            ext.input_schema.clone(),
        ));
    }

    let tools: Vec<ToolInfo> = session
        .tool_definitions
        .iter()
        .cloned()
        .map(ToolInfo::from)
        .collect();

    info!(
        session_id = %session.session_id,
        model = %session.model,
        tool_count = tools.len(),
        "Session initialized"
    );

    let _ = outbound_tx.send(OutboundMessage::SessionReady(SessionReady {
        session_id: session.session_id.clone(),
        tools,
    }));
}

/// Prepare and spawn a turn as a background task, returning an `ActiveTurn`
/// that the main loop can select on alongside the WebSocket receiver.
///
/// Returns `None` if the session is not initialized (an error is sent on the
/// outbound channel in that case).
fn start_turn(
    session: &mut Session,
    msg: UserMessage,
    outbound_tx: &mpsc::UnboundedSender<OutboundMessage>,
    ctx: &WsContext,
) -> Option<ActiveTurn> {
    if !session.initialized {
        let _ = outbound_tx.send(OutboundMessage::Error(ErrorMsg {
            code: "not_initialized".into(),
            message: "Send session_init before user_message".into(),
            recoverable: true,
        }));
        return None;
    }

    let message_id = Uuid::new_v4().to_string();
    let _ = outbound_tx.send(OutboundMessage::AssistantMessageStart(
        AssistantMessageStart {
            message_id: message_id.clone(),
        },
    ));

    session.messages.push(Message::user(&msg.content));

    let mut tool_executor = ToolExecutor::new(ctx.tool_config.clone());
    for ext in &session.external_tools {
        tool_executor.register_external(ext.clone());
    }
    let mut executor_router = ExecutorRouter::new();
    executor_router.add_executor(Arc::new(tool_executor));

    let workspace = session.workspace.join(session.agent_id.to_hex());
    let kernel_executor = KernelToolExecutor::new(executor_router, session.agent_id, workspace);

    let config = session.agent_loop_config();
    let agent_loop = AgentLoop::new(config);

    let tools = session.tool_definitions.clone();
    let messages = session.messages.clone();
    let provider = ctx.provider.clone();

    let cancel_token = CancellationToken::new();
    let cancel_clone = cancel_token.clone();

    let outbound_for_turn = outbound_tx.clone();

    let join_handle = tokio::spawn(async move {
        tokio::select! {
            biased;
            () = cancel_clone.cancelled() => {
                Ok(AgentLoopResult { timed_out: true, ..AgentLoopResult::default() })
            }
            result = agent_loop.run(provider.as_ref(), &kernel_executor, messages, tools) => {
                if let Ok(ref r) = result {
                    if !r.total_text.is_empty() {
                        let _ = outbound_for_turn.send(OutboundMessage::TextDelta(TextDelta {
                            text: r.total_text.clone(),
                        }));
                    }
                }
                result
            }
        }
    });

    let stream_forward_handle = tokio::spawn(async {});

    Some(ActiveTurn {
        cancel_token,
        join_handle,
        stream_forward_handle,
        message_id,
    })
}

/// Process the result of a completed (or cancelled) turn and update session state.
fn finalize_turn(
    session: &mut Session,
    join_result: Result<anyhow::Result<AgentLoopResult>, tokio::task::JoinError>,
    message_id: &str,
    outbound_tx: &mpsc::UnboundedSender<OutboundMessage>,
) {
    let result = match join_result {
        Ok(inner) => inner,
        Err(e) => {
            error!(session_id = %session.session_id, error = %e, "Turn task panicked");
            let _ = outbound_tx.send(OutboundMessage::Error(ErrorMsg {
                code: "internal_error".into(),
                message: "Turn processing task panicked".into(),
                recoverable: false,
            }));
            let _ = outbound_tx.send(OutboundMessage::AssistantMessageEnd(AssistantMessageEnd {
                message_id: message_id.to_string(),
                stop_reason: "error".into(),
                usage: SessionUsage::default(),
                files_changed: FilesChanged::default(),
            }));
            return;
        }
    };

    match result {
        Ok(loop_result) => {
            session.messages = loop_result.messages;

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

            let _ = outbound_tx.send(OutboundMessage::AssistantMessageEnd(AssistantMessageEnd {
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
                cumulative_in = session.cumulative_input_tokens,
                cumulative_out = session.cumulative_output_tokens,
                "Turn complete"
            );
        }
        Err(e) => {
            error!(session_id = %session.session_id, error = %e, "Turn processing failed");
            let _ = outbound_tx.send(OutboundMessage::Error(ErrorMsg {
                code: "turn_error".into(),
                message: format!("Turn processing failed: {e}"),
                recoverable: true,
            }));
        }
    }
}
