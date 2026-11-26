//! Idle-phase handling: waiting for the next command and starting turns.

use super::turn::start_turn;
use super::{respond_tool_approval, ActiveTurn};
use crate::gateway::session::chat_run::CHAT_RUN_IDLE_RETENTION;
use crate::gateway::session::generation;
use crate::gateway::session::{Session, WsContext};
use crate::protocol::{ErrorMsg, GenerationRequest, InboundMessage, OutboundMessage, UserMessage};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

pub(super) enum IdleAction {
    StartTurn(ActiveTurn),
    Close,
    Continue,
}

/// Wait (idle) for the next inbound command, an explicit shutdown, or
/// the idle-retention deadline.
///
/// When the run sits idle with no command for [`CHAT_RUN_IDLE_RETENTION`],
/// it is reaped only if no client is currently attached (`attach_count
/// == 0`); otherwise the wait re-arms so an attached-but-quiet client
/// keeps the run alive (plan Part C decision ii).
pub(super) async fn drive_idle(
    commands_rx: &mut mpsc::Receiver<InboundMessage>,
    session: &mut Session,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    ctx: &WsContext,
    attach_count: &Arc<AtomicUsize>,
    shutdown: &CancellationToken,
) -> IdleAction {
    let mailbox = session.agent_mailbox.clone();
    if let Some(message) = mailbox.pop() {
        return start_agent_message_turn(message, session, outbound_tx, ctx).await;
    }
    tokio::select! {
        biased;
        () = shutdown.cancelled() => {
            info!(session_id = %session.session_id, "Chat run stopped while idle");
            IdleAction::Close
        }
        () = mailbox.ready() => match mailbox.pop() {
            Some(message) => start_agent_message_turn(message, session, outbound_tx, ctx).await,
            None => IdleAction::Continue,
        },
        result = tokio::time::timeout(CHAT_RUN_IDLE_RETENTION, commands_rx.recv()) => {
            match result {
                Ok(Some(msg)) => dispatch_idle_message(msg, session, outbound_tx, ctx).await,
                // Command channel closed (registry entry removed and all
                // attaches gone): nothing can drive this run further.
                Ok(None) => IdleAction::Close,
                Err(_elapsed) => {
                    if attach_count.load(Ordering::Acquire) == 0 {
                        info!(
                            session_id = %session.session_id,
                            "Chat run idle with no attached clients past retention; reaping"
                        );
                        IdleAction::Close
                    } else {
                        IdleAction::Continue
                    }
                }
            }
        }
    }
}

/// Run a turn for a message from an agent this session spawned.
async fn start_agent_message_turn(
    message: aura_engine::InboxMessage,
    session: &mut Session,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    ctx: &WsContext,
) -> IdleAction {
    let msg = UserMessage {
        content: message.render(),
        tool_hints: None,
        attachments: None,
    };
    match start_turn(session, msg, outbound_tx, ctx).await {
        Some(turn) => IdleAction::StartTurn(ActiveTurn::Agent(turn)),
        None => IdleAction::Continue,
    }
}

async fn dispatch_idle_message(
    msg: InboundMessage,
    session: &mut Session,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    ctx: &WsContext,
) -> IdleAction {
    match msg {
        InboundMessage::UserMessage(msg) => {
            match start_turn(session, msg, outbound_tx, ctx).await {
                Some(turn) => IdleAction::StartTurn(ActiveTurn::Agent(turn)),
                None => IdleAction::Continue,
            }
        }
        InboundMessage::GenerationRequest(req) => {
            start_generation_request(&req, session, outbound_tx, ctx)
        }
        InboundMessage::Cancel => {
            debug!(session_id = %session.session_id, "Cancel received but no turn is active");
            IdleAction::Continue
        }
        InboundMessage::ApprovalResponse(resp) => {
            debug!(
                session_id = %session.session_id,
                tool_use_id = %resp.tool_use_id,
                approved = resp.approved,
                "Approval response received (not yet implemented)"
            );
            IdleAction::Continue
        }
        InboundMessage::ToolApprovalResponse(resp) => {
            respond_tool_approval(resp, session, outbound_tx);
            IdleAction::Continue
        }
    }
}

/// Start a generation turn, or reject it when no router URL is configured.
fn start_generation_request(
    req: &GenerationRequest,
    session: &mut Session,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    ctx: &WsContext,
) -> IdleAction {
    info!(
        session_id = %session.session_id,
        mode = %req.mode,
        has_router_url = ctx.router_url.is_some(),
        "Generation request received"
    );
    match ctx.router_url {
        Some(ref url) => match generation::start_generation(session, req, outbound_tx, url) {
            Some(turn) => IdleAction::StartTurn(ActiveTurn::Generation(turn)),
            None => IdleAction::Continue,
        },
        None => {
            warn!(
                session_id = %session.session_id,
                mode = %req.mode,
                "Generation request rejected because AURA_ROUTER_URL is not configured"
            );
            crate::inbound_console::ws_rejection_line(
                "framing",
                "no_router_url",
                Some(&format!("session={} mode={}", session.session_id, req.mode)),
            );
            let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
                code: "no_router_url".into(),
                message: "AURA_ROUTER_URL not configured; generation unavailable".into(),
                recoverable: false,
                support_id: None,
            }));
            IdleAction::Continue
        }
    }
}
