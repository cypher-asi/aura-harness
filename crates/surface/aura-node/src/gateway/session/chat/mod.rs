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

#[cfg(doc)]
use super::chat_run::CHAT_RUN_IDLE_RETENTION;
use super::chat_run::{ChatEventChannel, RunControl};
use super::generation::GenerationTurn;
use super::helpers;
use super::{Session, ToolApprovalBroker, WsContext};
use crate::protocol::{ChatTitle, ErrorMsg, InboundMessage, OutboundMessage, ToolApprovalResponse};
use aura_agent::{AgentLoopConfig, AgentLoopResult, KernelModelGateway};
use aura_agent_kernel::Kernel;
use aura_model::Message;
use idle::{drive_idle, IdleAction};
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
    /// The turn's kernel, which records its reply when it ends.
    kernel: Arc<Kernel>,
    /// The chat's title being written beside the turn, when the request
    /// asked for one.
    title: Option<TitleTask>,
}

/// A chat title being written through the turn's kernel and sent as soon
/// as the model gives one. Stopped when the turn is dropped unfinished, so
/// nothing is written for a run that was closed (an agent being sealed
/// must not gain entries afterwards).
struct TitleTask(JoinHandle<()>);

impl TitleTask {
    fn spawn(
        gateway: KernelModelGateway,
        config: AgentLoopConfig,
        model: String,
        prompt: String,
        outbound_tx: mpsc::Sender<OutboundMessage>,
    ) -> Self {
        Self(tokio::spawn(async move {
            if let Some(title) =
                aura_agent::titles::chat_title(&gateway, &config, &model, &prompt).await
            {
                helpers::send_outbound_with_backpressure(
                    &outbound_tx,
                    OutboundMessage::ChatTitle(ChatTitle { title }),
                    "chat_title",
                )
                .await;
            }
        }))
    }

    async fn finish(mut self) {
        let _ = (&mut self.0).await;
    }
}

impl Drop for TitleTask {
    fn drop(&mut self) {
        self.0.abort();
    }
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
/// stop via `control.shutdown`. `control.activity` holds the turn under
/// way.
pub(super) async fn run_chat_driver(
    mut session: Session,
    ctx: WsContext,
    events: Arc<ChatEventChannel>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    mut commands_rx: mpsc::Receiver<InboundMessage>,
    control: RunControl,
) {
    session.tool_approval_broker = Some(Arc::new(ToolApprovalBroker::new(outbound_tx.clone())));
    info!(session_id = %session.session_id, "Chat run driver started");

    // Bootstrap kernel-side state (SessionStart transaction + capability
    // install record + scheduler identity registration) and emit
    // `SessionReady` into the event channel so every (re)attaching
    // client replays it before live traffic.
    helpers::emit_session_ready(&mut session, &outbound_tx, &ctx).await;

    let active_turn =
        drive_turns(&mut commands_rx, &mut session, &outbound_tx, &ctx, &control).await;

    // Teardown: cancel any in-flight turn so its background task stops
    // promptly, then mark the channel done so a late attach replays
    // history without waiting on a live receiver that will never fire.
    match active_turn {
        Some(ActiveTurn::Agent(agent)) => agent.cancel_token.cancel(),
        Some(ActiveTurn::Generation(gen)) => gen.cancel_token.cancel(),
        None => {}
    }
    control.activity.end();
    events.mark_done();
    info!(session_id = %session.session_id, "Chat run driver stopped");
    drop(outbound_tx);
}

/// The driver's turn loop: alternate between waiting for a turn and
/// driving it until the run closes, keeping `control.activity` on the
/// turn under way. Returns the turn still under way at close, if any.
async fn drive_turns(
    commands_rx: &mut mpsc::Receiver<InboundMessage>,
    session: &mut Session,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    ctx: &WsContext,
    control: &RunControl,
) -> Option<ActiveTurn> {
    let RunControl {
        attach_count,
        shutdown,
        activity,
    } = control;
    let mut active_turn: Option<ActiveTurn> = None;

    loop {
        if let Some(ref mut turn) = active_turn {
            let action = drive_active_turn(commands_rx, turn, session, outbound_tx, shutdown).await;
            match action {
                TurnAction::TurnFinished => {
                    active_turn = None;
                    activity.end();
                }
                TurnAction::Close => break,
                TurnAction::Continue => {}
            }
        } else {
            match drive_idle(
                commands_rx,
                session,
                outbound_tx,
                ctx,
                attach_count,
                shutdown,
            )
            .await
            {
                IdleAction::StartTurn(turn) => {
                    activity.begin(turn_label(session, &turn));
                    active_turn = Some(turn);
                }
                IdleAction::Close => break,
                IdleAction::Continue => {}
            }
        }
    }
    active_turn
}

/// Longest turn label, in characters.
const TURN_LABEL_CHARS: usize = 120;

/// What `turn` is called in PROCESSES: its prompt's first line, which
/// starting it appended to `session`'s messages.
fn turn_label(session: &Session, turn: &ActiveTurn) -> String {
    let prompt = match turn {
        ActiveTurn::Agent(_) => session
            .messages
            .last()
            .map(Message::text_content)
            .unwrap_or_default(),
        ActiveTurn::Generation(_) => return "Generating media".into(),
    };
    let line = prompt.lines().map(str::trim).find(|line| !line.is_empty());
    line.map_or_else(
        || "Turn".into(),
        |line| line.chars().take(TURN_LABEL_CHARS).collect(),
    )
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
    // The title goes out before the turn's end, which is where a client
    // stops reading. A cancelled turn ends without waiting for it.
    if let Some(title) = agent.title.take() {
        if !agent.cancel_token.is_cancelled() {
            title.finish().await;
        }
    }
    helpers::finalize_turn(
        session,
        join_result,
        &message_id,
        outbound_tx,
        Some(agent.kernel.as_ref()),
    )
    .await;
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
