//! The WS side of a chat run: each attached socket replays the run's
//! history, then follows it live, while forwarding its inbound frames
//! to the run's driver.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use super::{ChatEventSubscription, ChatRunHandle};
use crate::protocol::{ErrorMsg, InboundMessage, OutboundMessage};

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
