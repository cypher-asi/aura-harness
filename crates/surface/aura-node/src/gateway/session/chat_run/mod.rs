//! Reattachable chat-run registry and per-run event channel.
//!
//! Mirrors the automaton-side
//! [`aura_engine::automaton::EventChannel`] model so a chat run's
//! turn execution can be decoupled from any single WebSocket
//! connection. Where the automaton channel replays
//! [`aura_engine_automaton::AutomatonEvent`]s, this one replays
//! [`OutboundMessage`]s (the chat wire protocol).
//!
//! Motivation: today a chat run is one-shot — the first
//! `WS /stream/:run_id` consumes the prepared [`super::Session`] and a
//! WS close cancels the in-flight turn, so a dropped server↔harness
//! socket kills the turn and a reconnect 409s. By giving each run an
//! independent driver task that owns the `Session` and emits into this
//! channel, a dropped WS can be re-established: a reattaching client
//! replays the run's history and then continues live, exactly like the
//! automaton `/stream/:run_id` path.
//!
//! Concurrency model:
//! - [`ChatEventChannel::push`] appends to the replay `history` and
//!   broadcasts the same message while holding the history lock, so a
//!   concurrent [`ChatEventChannel::subscribe`] (which also takes the
//!   lock before snapshotting history + creating its live receiver)
//!   observes each message exactly once — never both in the replayed
//!   history and again on the live receiver, and never dropped between
//!   the two.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use super::{Session, WsContext};
use crate::protocol::{ErrorMsg, InboundMessage, OutboundMessage};

/// Live broadcast ring capacity. A slow attached client that lags past
/// this many buffered messages observes `RecvError::Lagged`; the WS
/// adapter surfaces that as a progress/warning frame rather than
/// silently dropping content.
const CHAT_EVENT_BROADCAST_CAPACITY: usize = 512;

/// Cap on the per-run replay buffer. Mirrors the broadcast ring so an
/// attached client that keeps up and a late attach that relies on
/// replay observe the same visible window. Exceeding the cap drops the
/// oldest entries first (whole-session history with monotonic ordering;
/// see plan Part C decision iv).
const CHAT_EVENT_HISTORY_CAPACITY: usize = CHAT_EVENT_BROADCAST_CAPACITY;

/// How long the driver lets a run sit fully idle — no active turn and
/// no attached client — before reaping it from the registry. Provides
/// the reconnect grace window for a dropped server↔harness socket
/// (plan Part C decision ii: one `run_id` per chat-session lifetime,
/// removed after this window with no active attaches).
pub(crate) const CHAT_RUN_IDLE_RETENTION: Duration = Duration::from_secs(300);

/// Replay-aware broadcast channel for a single chat run.
pub(crate) struct ChatEventChannel {
    /// Replay history, capped at [`CHAT_EVENT_HISTORY_CAPACITY`]
    /// (oldest-first eviction). Cloned on each [`Self::subscribe`].
    history: Mutex<Vec<OutboundMessage>>,
    /// Live broadcast for currently-attached clients. Retained inside
    /// the `Arc<ChatEventChannel>` so the sender outlives the driver's
    /// forwarder task and late subscribers don't see `Closed` before
    /// draining history.
    broadcast: broadcast::Sender<OutboundMessage>,
    /// Set once the run's driver has stopped. Lets a late attach skip
    /// the live-receive loop after replaying history.
    done: AtomicBool,
}

/// Snapshot returned by [`ChatEventChannel::subscribe`]: replay
/// `history` (consume first, in order) plus a `live` receiver (consume
/// next). Produces the same ordering an early subscriber would have
/// seen, with each message delivered exactly once across the two.
pub(crate) struct ChatEventSubscription {
    pub(crate) history: Vec<OutboundMessage>,
    pub(crate) live: broadcast::Receiver<OutboundMessage>,
    pub(crate) already_done: bool,
}

impl ChatEventChannel {
    pub(crate) fn new() -> Arc<Self> {
        let (broadcast_tx, _) = broadcast::channel(CHAT_EVENT_BROADCAST_CAPACITY);
        Arc::new(Self {
            history: Mutex::new(Vec::new()),
            broadcast: broadcast_tx,
            done: AtomicBool::new(false),
        })
    }

    /// Append a message to the replay history and broadcast it to live
    /// subscribers. History append and broadcast happen under the same
    /// lock so [`Self::subscribe`] sees each message exactly once.
    pub(crate) fn push(&self, msg: OutboundMessage) {
        let mut history = self
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if history.len() >= CHAT_EVENT_HISTORY_CAPACITY {
            let drop_n = history.len() + 1 - CHAT_EVENT_HISTORY_CAPACITY;
            history.drain(..drop_n);
        }
        history.push(msg.clone());
        // Broadcast while holding the history lock so a concurrent
        // `subscribe` (which takes the same lock before creating its
        // receiver) cannot interleave between the append and the send.
        let _ = self.broadcast.send(msg);
    }

    pub(crate) fn subscribe(&self) -> ChatEventSubscription {
        let history = self
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Create the live receiver while holding the history lock; see
        // `push` for the exactly-once ordering argument.
        let live = self.broadcast.subscribe();
        let snapshot = history.clone();
        drop(history);
        ChatEventSubscription {
            history: snapshot,
            live,
            already_done: self.done.load(Ordering::Acquire),
        }
    }

    pub(crate) fn mark_done(&self) {
        self.done.store(true, Ordering::Release);
    }
}

/// Spawn the per-run forwarder that drains the driver's outbound mpsc
/// into the replay history + live broadcast. Returns the sender the
/// driver (and its turn tasks / approval broker) clone to emit
/// outbound messages.
pub(crate) fn spawn_event_forwarder(
    channel: Arc<ChatEventChannel>,
) -> mpsc::Sender<OutboundMessage> {
    let (tx, mut rx) = mpsc::channel::<OutboundMessage>(1024);
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            channel.push(msg);
        }
    });
    tx
}

/// Parent-linkage metadata for a registered run.
///
/// `None` on a top-level run (registered by `POST /v1/run`); `Some` on
/// a child subagent run so the shared registry exposes the same
/// parent/child lineage a real top-level run would have — letting a
/// child be looked up, attached, and cancelled through the same
/// mechanisms while still carrying its ancestry.
#[derive(Debug, Clone)]
pub(crate) struct RunLinkage {
    /// Run id of the parent (top-level) chat run that spawned this
    /// child. `None` when the parent run id was not threaded (e.g. the
    /// bootstrap/test paths that build a hook without a live run).
    pub(crate) parent_run_id: Option<String>,
    /// Parent tool-call id (`toolu_…`) of the `task` invocation that
    /// produced this child, tying the run back to its spawning tool use.
    pub(crate) parent_tool_use_id: Option<String>,
    /// This run's own id (the registry key), retained on the handle so
    /// linkage is self-describing without a registry round-trip.
    pub(crate) child_run_id: String,
    /// Nesting depth = number of ancestors in [`Self::parent_chain`].
    pub(crate) depth: usize,
    /// Ancestor agent-id chain `[parent, grandparent, …]` (stringified).
    pub(crate) parent_chain: Vec<String>,
}

/// Registry entry for a live chat run. Shared between the registry
/// (`RouterState::chat_runs`), the driver task, and every attached WS
/// adapter.
pub(crate) struct ChatRunHandle {
    /// Inbound command channel feeding the driver. WS adapters clone
    /// this to forward parsed [`crate::protocol::InboundMessage`]s.
    pub(crate) commands: mpsc::Sender<crate::protocol::InboundMessage>,
    /// Replay-aware outbound event channel for attaching clients.
    pub(crate) events: Arc<ChatEventChannel>,
    /// Number of currently-attached WS adapters. The driver consults
    /// this to decide whether an idle run may be reaped.
    pub(crate) attach_count: Arc<AtomicUsize>,
    /// Explicit-stop signal. Cancelled by `POST /v1/run/:id/stop`; the
    /// driver watches it to tear down (cancelling any active turn). For
    /// a child run this token is forked from the parent turn token, so
    /// stopping the child cancels only that child while a parent-turn
    /// cancellation still propagates down.
    pub(crate) shutdown: CancellationToken,
    /// Parent-linkage metadata. `None` for a top-level run, `Some` for a
    /// child subagent run.
    pub(crate) linkage: Option<RunLinkage>,
}

/// `run_id` → live chat run.
pub(crate) type ChatRunRegistry = Arc<DashMap<String, Arc<ChatRunHandle>>>;

/// Pieces needed to build + register a [`ChatRunHandle`]. Shared by the
/// top-level `POST /v1/run` path ([`spawn_chat_run`]) and the child
/// subagent path
/// ([`super::subagent_stream::RuntimeSubagentObservabilityHook`]) so
/// both register through the exact same run-handle path instead of
/// constructing the handle independently.
pub(crate) struct RunRegistration {
    pub(crate) commands: mpsc::Sender<crate::protocol::InboundMessage>,
    pub(crate) events: Arc<ChatEventChannel>,
    pub(crate) attach_count: Arc<AtomicUsize>,
    pub(crate) shutdown: CancellationToken,
    pub(crate) linkage: Option<RunLinkage>,
}

/// Build a [`ChatRunHandle`] from `reg` and insert it into `registry`
/// under `run_id`, returning the shared handle. Single insertion point
/// for both top-level and child runs.
pub(crate) fn register_run(
    registry: &ChatRunRegistry,
    run_id: String,
    reg: RunRegistration,
) -> Arc<ChatRunHandle> {
    let handle = Arc::new(ChatRunHandle {
        commands: reg.commands,
        events: reg.events,
        attach_count: reg.attach_count,
        shutdown: reg.shutdown,
        linkage: reg.linkage,
    });
    registry.insert(run_id, handle.clone());
    handle
}

/// RAII guard that tracks one WS attach against a run's `attach_count`.
/// Incremented on construction, decremented on drop, so the driver's
/// idle-reaper sees an accurate live-attach count even if the adapter
/// task unwinds.
pub(crate) struct AttachGuard {
    count: Arc<AtomicUsize>,
}

impl AttachGuard {
    pub(crate) fn new(count: Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        Self { count }
    }
}

impl Drop for AttachGuard {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Spawn a [`super::chat::run_chat_driver`] task that owns `session`
/// and its turn execution, register it under `run_id`, and return the
/// shared [`ChatRunHandle`]. The driver runs independently of any
/// WebSocket; it removes itself from the registry when it stops.
pub(crate) fn spawn_chat_run(
    session: Session,
    mut ctx: WsContext,
    run_id: String,
    registry: ChatRunRegistry,
) -> Arc<ChatRunHandle> {
    // Make the run id visible to the per-turn kernel build so the
    // subagent observability hook can stamp `parent_run_id` onto each
    // child run's linkage metadata.
    ctx.run_id = Some(run_id.clone());

    let events = ChatEventChannel::new();
    let outbound_tx = spawn_event_forwarder(events.clone());
    let (commands_tx, commands_rx) = mpsc::channel::<InboundMessage>(256);
    let attach_count = Arc::new(AtomicUsize::new(0));
    let shutdown = CancellationToken::new();

    let handle = register_run(
        &registry,
        run_id.clone(),
        RunRegistration {
            commands: commands_tx,
            events: events.clone(),
            attach_count: attach_count.clone(),
            shutdown: shutdown.clone(),
            // Top-level run: no parent linkage.
            linkage: None,
        },
    );

    let registry_for_task = registry;
    tokio::spawn(async move {
        super::chat::run_chat_driver(
            session,
            ctx,
            events,
            outbound_tx,
            commands_rx,
            attach_count,
            shutdown,
        )
        .await;
        // Driver stopped: drop the registry entry so no new attach finds
        // a dead run. Clients that already subscribed keep their (now
        // `done`) channel until they drop.
        registry_for_task.remove(&run_id);
    });

    handle
}

/// Thin WS adapter for a chat run: replay the run's history, then
/// stream live events, while forwarding inbound frames to the driver's
/// command channel. Multiple concurrent attaches to the same run are
/// allowed. A WS close ends only this attach — it never cancels the
/// turn (plan Part C).
pub(crate) async fn handle_chat_ws_attach(
    socket: axum::extract::ws::WebSocket,
    handle: Arc<ChatRunHandle>,
    run_id: String,
) {
    use futures_util::StreamExt;

    let (mut ws_tx, ws_rx) = socket.split();
    // Track this attach for the driver's idle-reaper.
    let _attach = AttachGuard::new(handle.attach_count.clone());
    let ChatEventSubscription {
        history,
        mut live,
        already_done,
    } = handle.events.subscribe();

    tracing::info!(
        run_id = %run_id,
        history_len = history.len(),
        already_done,
        "Chat run WS attached"
    );

    // Per-attach control channel: lets the reader push frames addressed
    // to *this* socket only (e.g. a `parse_error` for malformed inbound
    // JSON) without broadcasting them into the run's shared replay
    // history, which every other attached client would otherwise see.
    let (ctrl_tx, mut ctrl_rx) = mpsc::channel::<OutboundMessage>(8);

    // Reader task: forward inbound frames to the driver. On WS close it
    // signals `attach_closed` (so the writer stops) but does NOT cancel
    // the turn.
    let attach_closed = CancellationToken::new();
    let reader = tokio::spawn(run_attach_reader(
        ws_rx,
        handle.commands.clone(),
        ctrl_tx,
        attach_closed.clone(),
        run_id.clone(),
    ));

    // Writer: replay history first (exactly the messages an early
    // attach would have seen), then live until the WS closes or the run
    // terminates.
    let mut closed = false;
    for msg in history {
        if send_outbound_frame(&mut ws_tx, &msg).await.is_err() {
            closed = true;
            break;
        }
    }

    if !closed && !already_done {
        write_live(&mut ws_tx, &attach_closed, &mut ctrl_rx, &mut live).await;
    }

    reader.abort();
    tracing::info!(run_id = %run_id, "Chat run WS attach ended");
}

/// Forward inbound WS frames to the driver's command channel until the
/// socket closes or the driver goes away, then cancel `attach_closed`.
async fn run_attach_reader(
    mut ws_rx: futures_util::stream::SplitStream<axum::extract::ws::WebSocket>,
    commands: mpsc::Sender<InboundMessage>,
    ctrl_tx: mpsc::Sender<OutboundMessage>,
    attach_closed: CancellationToken,
    run_id: String,
) {
    use axum::extract::ws::Message as WsMessage;
    use futures_util::StreamExt;
    while let Some(frame) = ws_rx.next().await {
        match frame {
            Ok(WsMessage::Text(text)) => {
                let keep_going = match serde_json::from_str::<InboundMessage>(&text) {
                    // Driver gone; nothing left to forward.
                    Ok(inbound) => commands.send(inbound).await.is_ok(),
                    Err(e) => reject_inbound_frame(&ctrl_tx, &run_id, &e).await,
                };
                if !keep_going {
                    break;
                }
            }
            Ok(WsMessage::Close(_)) | Err(_) => break,
            _ => {}
        }
    }
    attach_closed.cancel();
}

/// Log a malformed inbound frame and tell *this* client it was
/// rejected. Returns `false` when the writer is gone.
async fn reject_inbound_frame(
    ctrl_tx: &mpsc::Sender<OutboundMessage>,
    run_id: &str,
    e: &serde_json::Error,
) -> bool {
    crate::inbound_console::ws_rejection_line(
        "framing",
        "parse_error",
        Some(&format!("run_id={run_id} {e}")),
    );
    // Recoverable: the socket stays open so the client can resend a
    // well-formed message.
    ctrl_tx
        .send(OutboundMessage::Error(ErrorMsg {
            code: "parse_error".into(),
            message: format!("failed to parse inbound message: {e}"),
            recoverable: true,
            support_id: None,
        }))
        .await
        .is_ok()
}

/// Stream per-attach control frames and live broadcast traffic to the
/// socket until the attach closes, the socket fails or the run ends.
async fn write_live<S>(
    ws_tx: &mut S,
    attach_closed: &CancellationToken,
    ctrl_rx: &mut mpsc::Receiver<OutboundMessage>,
    live: &mut broadcast::Receiver<OutboundMessage>,
) where
    S: futures_util::Sink<axum::extract::ws::Message> + Unpin,
{
    loop {
        tokio::select! {
            biased;
            () = attach_closed.cancelled() => break,
            // Per-attach control frames (e.g. `parse_error`) go only
            // to this socket, ahead of live broadcast traffic.
            Some(ctrl) = ctrl_rx.recv() => {
                if send_outbound_frame(ws_tx, &ctrl).await.is_err() {
                    break;
                }
            }
            recv = live.recv() => match recv {
                Ok(msg) => {
                    if send_outbound_frame(ws_tx, &msg).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    let warn = OutboundMessage::Progress(crate::protocol::ProgressMsg {
                        stage: "lagged".into(),
                        tool_name: None,
                        elapsed_ms: None,
                        message: Some(format!("dropped {n} messages (client too slow)")),
                        reset_text_bytes: None,
                        reset_thinking_bytes: None,
                    });
                    let _ = send_outbound_frame(ws_tx, &warn).await;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}

/// Serialize and send one outbound message over the WS sink. Returns
/// `Err(())` only when the socket itself failed (so the writer should
/// stop); a serialization failure is logged and skipped.
async fn send_outbound_frame<S>(sink: &mut S, msg: &OutboundMessage) -> Result<(), ()>
where
    S: futures_util::Sink<axum::extract::ws::Message> + Unpin,
{
    use axum::extract::ws::Message as WsMessage;
    use futures_util::SinkExt;
    match serde_json::to_string(msg) {
        Ok(json) => sink.send(WsMessage::Text(json)).await.map_err(|_| ()),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to serialize outbound chat message");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod driver_tests;
