//! Chat run driver and turn management.
//!
//! Part C: the chat turn loop is decoupled from any single WebSocket.
//! [`run_chat_driver`] owns the [`Session`] and runs the turn loop in a
//! background task spawned by `POST /v1/run` (see
//! [`super::chat_run::spawn_chat_run`]). Inbound commands arrive over an
//! mpsc fed by the WS adapter ([`super::chat_run::handle_chat_ws_attach`]);
//! outbound frames are emitted into a replay-aware
//! [`super::chat_run::ChatEventChannel`] so a dropped server↔harness
//! socket can reattach (history replay + live) without killing the turn.
//!
//! Two behavior changes vs the pre-Part-C one-shot WS handler:
//! - A WS close NEVER cancels the active turn; it only ends that attach.
//!   Cancellation is explicit (`InboundMessage::Cancel`) or via run stop
//!   (the driver's `shutdown` token).
//! - WS framing/transport errors are handled per-attach by the adapter
//!   and are not broadcast as `Error` frames to other attached clients.

mod attachments;
mod idle;
mod turn;

#[cfg(test)]
mod tests;

pub(super) use turn::populate_tool_definitions;

use super::chat_run::ChatEventChannel;
#[cfg(doc)]
use super::chat_run::CHAT_RUN_IDLE_RETENTION;
use super::generation::GenerationTurn;
use super::helpers;
use super::{Session, ToolApprovalBroker, WsContext};
use crate::protocol::{ErrorMsg, InboundMessage, OutboundMessage, ToolApprovalResponse};
use aura_agent::AgentLoopResult;
use idle::{drive_idle, IdleAction};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

/// State for a turn that is currently being processed in the background.
enum ActiveTurn {
    Agent(AgentTurn),
    Generation(GenerationTurn),
}

struct AgentTurn {
    cancel_token: CancellationToken,
    join_handle: JoinHandle<anyhow::Result<AgentLoopResult>>,
    stream_forward_handle: JoinHandle<()>,
    message_id: String,
}

/// Drive a chat run's full turn lifecycle, decoupled from any WS.
///
/// `session` is already populated by [`super::prepare_chat_session`].
/// The driver attaches the approval broker (wired to the run's event
/// channel), emits `SessionReady` into the replay history, then loops:
/// it consumes [`InboundMessage`]s from `commands_rx` (fed by every WS
/// adapter) and emits outbound frames into `outbound_tx` (which the
/// forwarder fans out to history + live subscribers).
///
/// Lifecycle (plan Part C decisions ii/v): the run survives WS drops;
/// it is reaped only when fully idle (no active turn) with zero
/// attached clients for [`CHAT_RUN_IDLE_RETENTION`], or on explicit
/// stop via `shutdown`.
pub(super) async fn run_chat_driver(
    mut session: Session,
    ctx: WsContext,
    events: Arc<ChatEventChannel>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    mut commands_rx: mpsc::Receiver<InboundMessage>,
    attach_count: Arc<AtomicUsize>,
    shutdown: CancellationToken,
) {
    session.tool_approval_broker = Some(Arc::new(ToolApprovalBroker::new(outbound_tx.clone())));
    info!(session_id = %session.session_id, "Chat run driver started");

    // Bootstrap kernel-side state (SessionStart transaction + capability
    // install record + scheduler identity registration) and emit
    // `SessionReady` into the event channel so every (re)attaching
    // client replays it before live traffic.
    helpers::emit_session_ready(&mut session, &outbound_tx, &ctx).await;

    let mut active_turn: Option<ActiveTurn> = None;

    loop {
        if let Some(ref mut turn) = active_turn {
            let action = drive_active_turn(
                &mut commands_rx,
                turn,
                &mut session,
                &outbound_tx,
                &shutdown,
            )
            .await;
            match action {
                TurnAction::TurnFinished => active_turn = None,
                TurnAction::Close => break,
                TurnAction::Continue => {}
            }
        } else {
            match drive_idle(
                &mut commands_rx,
                &mut session,
                &outbound_tx,
                &ctx,
                &attach_count,
                &shutdown,
            )
            .await
            {
                IdleAction::StartTurn(turn) => active_turn = Some(turn),
                IdleAction::Close => break,
                IdleAction::Continue => {}
            }
        }
    }

    // Teardown: cancel any in-flight turn so its background task stops
    // promptly, then mark the channel done so a late attach replays
    // history without waiting on a live receiver that will never fire.
    match active_turn {
        Some(ActiveTurn::Agent(agent)) => agent.cancel_token.cancel(),
        Some(ActiveTurn::Generation(gen)) => gen.cancel_token.cancel(),
        None => {}
    }
    events.mark_done();
    info!(session_id = %session.session_id, "Chat run driver stopped");
    drop(outbound_tx);
}

enum TurnAction {
    TurnFinished,
    Close,
    Continue,
}

/// Advance an active turn: react to one inbound command, the turn's
/// own completion, or an explicit shutdown.
///
/// Crucially, a closed command channel (`recv()` → `None`, i.e. every
/// WS adapter dropped and the run is being torn down) does NOT cancel
/// the turn here; the only cancellation paths are an explicit
/// `InboundMessage::Cancel` or `shutdown`.
async fn drive_active_turn(
    commands_rx: &mut mpsc::Receiver<InboundMessage>,
    turn: &mut ActiveTurn,
    session: &mut Session,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    shutdown: &CancellationToken,
) -> TurnAction {
    match turn {
        ActiveTurn::Agent(agent) => {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => {
                    info!(session_id = %session.session_id, "Chat run stopped during agent turn; cancelling turn");
                    agent.cancel_token.cancel();
                    TurnAction::Close
                }
                cmd = commands_rx.recv() => {
                    match cmd {
                        Some(msg) => {
                            handle_msg_during_turn(msg, &agent.cancel_token, session, outbound_tx);
                            TurnAction::Continue
                        }
                        // Command channel fully closed: registry entry
                        // removed AND every attach gone. Close the run
                        // (post-loop teardown cancels the turn).
                        None => TurnAction::Close,
                    }
                }
                join_result = &mut agent.join_handle => {
                    complete_agent_turn(join_result, agent, session, outbound_tx).await;
                    TurnAction::TurnFinished
                }
            }
        }
        ActiveTurn::Generation(gen) => {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => {
                    info!(session_id = %session.session_id, "Chat run stopped during generation; cancelling turn");
                    gen.cancel_token.cancel();
                    TurnAction::Close
                }
                cmd = commands_rx.recv() => {
                    match cmd {
                        Some(msg) => {
                            handle_msg_during_turn(msg, &gen.cancel_token, session, outbound_tx);
                            TurnAction::Continue
                        }
                        None => TurnAction::Close,
                    }
                }
                join_result = &mut gen.join_handle => {
                    log_generation_join(join_result, session);
                    TurnAction::TurnFinished
                }
            }
        }
    }
}

/// Await the agent turn's stream-forward task and finalize the turn
/// (emit `AssistantMessageEnd`, persist usage / messages).
async fn complete_agent_turn(
    join_result: Result<anyhow::Result<AgentLoopResult>, tokio::task::JoinError>,
    agent: &mut AgentTurn,
    session: &mut Session,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
) {
    let message_id = agent.message_id.clone();
    if let Err(e) = (&mut agent.stream_forward_handle).await {
        error!(
            session_id = %session.session_id,
            error = %e,
            "Stream forward task failed"
        );
    }
    helpers::finalize_turn(session, join_result, &message_id, outbound_tx).await;
}

fn log_generation_join(join_result: Result<(), tokio::task::JoinError>, session: &Session) {
    match join_result {
        Ok(()) => info!(session_id = %session.session_id, "Generation turn task finished"),
        Err(e) => error!(
            session_id = %session.session_id,
            error = %e,
            "Generation turn task failed"
        ),
    }
}

fn handle_msg_during_turn(
    msg: InboundMessage,
    cancel_token: &CancellationToken,
    session: &Session,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
) {
    match msg {
        InboundMessage::Cancel => {
            info!(session_id = %session.session_id, "Cancelling active turn");
            cancel_token.cancel();
        }
        InboundMessage::ToolApprovalResponse(resp) => {
            respond_tool_approval(resp, session, outbound_tx);
        }
        _ => {
            crate::inbound_console::ws_rejection_line(
                "framing",
                "turn_in_progress",
                Some(&format!("session={}", session.session_id)),
            );
            let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
                code: "turn_in_progress".into(),
                message: "A turn is currently in progress; send cancel first".into(),
                recoverable: true,
                support_id: None,
            }));
        }
    }
}

/// Forward a tool-approval response to the session's broker, reporting a
/// recoverable `approval_response_error` frame when the broker rejects it.
fn respond_tool_approval(
    resp: ToolApprovalResponse,
    session: &Session,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
) {
    if let Some(ref broker) = session.tool_approval_broker {
        if let Err(e) = broker.respond(resp) {
            crate::inbound_console::ws_rejection_line(
                "framing",
                "approval_response_error",
                Some(&format!("session={} {e}", session.session_id)),
            );
            let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
                code: "approval_response_error".into(),
                message: e,
                recoverable: true,
                support_id: None,
            }));
        }
    }
}
