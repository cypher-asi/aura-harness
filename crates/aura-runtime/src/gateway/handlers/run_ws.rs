//! WebSocket upgrade handlers for the gateway.
//!
//! Phase A note: the two pre-refactor paths `/stream` (chat WS with
//! `SessionInit` first frame) and `/stream/automaton/:id` (event-only
//! automaton stream) collapse into a single `/stream/:run_id` route.
//! Disambiguation happens by run id: chat runs sit in
//! [`super::RouterState::chat_runs`] (reattachable driver tasks);
//! automaton runs are tracked by the
//! [`aura_engine::automaton::AutomatonBridge`].

use super::super::*;
use axum::response::Response;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Maximum number of concurrent WebSocket connections this node will
/// serve at once. Each live socket holds a tokio task plus terminal /
/// session state; capping the count bounds the "slow-client task
/// exhaustion" worst case flagged by the H5 audit finding.
pub(in crate::gateway) const MAX_WS_CONNS_PER_NODE: usize = 128;

/// Try to reserve a WebSocket connection slot.
pub(in crate::gateway) fn try_acquire_ws_slot(
    sem: &Arc<Semaphore>,
) -> Option<OwnedSemaphorePermit> {
    Arc::clone(sem).try_acquire_owned().ok()
}

/// `WS /stream/:run_id` — bidirectional for chat runs created via
/// `POST /v1/run` with `RuntimeRequestType::Chat`; event-only for
/// DevLoop / TaskRun automaton runs.
///
/// Belt-and-suspenders bearer check (only when
/// [`crate::config::NodeConfig::require_auth`] is on). The router
/// middleware already rejects unauthenticated upgrades; this inline
/// guard prevents a regression if a future contributor wires the
/// handler up to a fresh `Router` that does not inherit the
/// middleware layer.
pub(in crate::gateway) async fn run_ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    Path(run_id): Path<String>,
    State(state): State<RouterState>,
) -> Response {
    if state.config.require_auth {
        if let Err(status) = crate::auth::check_bearer(&headers, &state.config.auth_token) {
            crate::inbound_console::ws_rejection_line(
                "upgrade.run",
                "unauthorized",
                Some(&format!("run_id={run_id}")),
            );
            return status.into_response();
        }
    }

    let Some(permit) = try_acquire_ws_slot(&state.ws_slots) else {
        warn!(
            cap = MAX_WS_CONNS_PER_NODE,
            run_id = %run_id,
            "Refusing /stream/:run_id upgrade: WS connection cap reached"
        );
        crate::inbound_console::ws_rejection_line(
            "upgrade.run",
            "slot_full",
            Some(&format!("cap={MAX_WS_CONNS_PER_NODE} run_id={run_id}")),
        );
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    // Part C dispatch: chat runs live in `chat_runs` (a driver task
    // owns the session); automaton runs are looked up via the bridge.
    // Chat runs take priority — the run-id allocator never collides
    // because chat runs use a freshly minted UUID and automaton runs
    // use their own (also UUID-shaped) ids. Unlike the pre-Part-C
    // one-shot path, attaching is non-destructive: multiple concurrent
    // attaches to the same run are allowed, and a dropped socket can
    // reattach (history replay + live) without killing the turn.
    if let Some(handle) = state
        .chat_runs
        .get(&run_id)
        .map(|entry| entry.value().clone())
    {
        return ws
            .on_upgrade(move |socket| async move {
                crate::gateway::session::handle_chat_ws_attach(socket, handle, run_id).await;
                drop(permit);
            })
            .into_response();
    }

    let bridge = match state.automaton_bridge.clone() {
        Some(b) => b,
        None => {
            crate::inbound_console::ws_rejection_line(
                "upgrade.run",
                "not_found",
                Some(&format!("run_id={run_id}")),
            );
            return StatusCode::NOT_FOUND.into_response();
        }
    };

    ws.on_upgrade(move |socket| async move {
        handle_automaton_ws(socket, run_id, bridge).await;
        drop(permit);
    })
    .into_response()
}

async fn handle_automaton_ws(
    socket: axum::extract::ws::WebSocket,
    automaton_id: String,
    bridge: Arc<AutomatonBridge>,
) {
    use axum::extract::ws::Message as WsMessage;
    use futures_util::{SinkExt, StreamExt};

    let (mut ws_tx, mut ws_rx) = socket.split();

    let subscription = match bridge.subscribe_events(&automaton_id) {
        Some(sub) => sub,
        None => {
            let msg = serde_json::json!({"type": "error", "message": format!("run {automaton_id} not found or already finished")}).to_string();
            let _: Result<(), _> = ws_tx.send(WsMessage::Text(msg)).await;
            return;
        }
    };
    info!(
        automaton_id = %automaton_id,
        history_len = subscription.history.len(),
        already_done = subscription.already_done,
        "Run event stream connected"
    );

    let drain_aid = automaton_id.clone();
    let drain_handle = tokio::spawn(async move {
        while let Some(msg) = ws_rx.next().await {
            match msg {
                Ok(WsMessage::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
        tracing::debug!(automaton_id = %drain_aid, "Run WS read side closed");
    });

    forward_automaton_subscription(&mut ws_tx, subscription, drain_handle).await;
    info!(automaton_id = %automaton_id, "Run event stream disconnected");
}

/// End the attach as soon as the reader closes, even when the run is
/// quiet or a slow client has blocked a replay/live write. Otherwise the
/// outer upgrade task retains its connection permit until another event.
async fn forward_automaton_subscription<S>(
    ws_tx: &mut S,
    subscription: aura_engine::automaton::EventSubscription,
    mut reader: tokio::task::JoinHandle<()>,
) where
    S: futures_util::Sink<axum::extract::ws::Message> + Unpin,
{
    use axum::extract::ws::Message as WsMessage;
    use futures_util::SinkExt;

    let aura_engine::automaton::EventSubscription {
        history,
        mut live,
        already_done,
    } = subscription;

    let forward = async {
        let mut saw_done_in_history = false;
        for event in history {
            let is_done = matches!(event, aura_surface_automaton::AutomatonEvent::Done);
            match serde_json::to_string(&event) {
                Ok(json) => {
                    if ws_tx.send(WsMessage::Text(json)).await.is_err() {
                        return;
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to serialize replayed automaton event");
                }
            }
            if is_done {
                saw_done_in_history = true;
                break;
            }
        }

        if !saw_done_in_history && !already_done {
            loop {
                match live.recv().await {
                    Ok(event) => {
                        let is_done = matches!(event, aura_surface_automaton::AutomatonEvent::Done);
                        match serde_json::to_string(&event) {
                            Ok(json) => {
                                if ws_tx.send(WsMessage::Text(json)).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Failed to serialize automaton event");
                            }
                        }
                        if is_done {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        let msg = serde_json::json!({"type": "warning", "message": format!("dropped {n} events (client too slow)")});
                        let _ = ws_tx.send(WsMessage::Text(msg.to_string())).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    };
    tokio::select! {
        biased;
        _ = &mut reader => {},
        () = forward => {},
    }
    reader.abort();
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_engine::automaton::EventSubscription;
    use std::time::Duration;
    use tokio::sync::{broadcast, oneshot};

    #[tokio::test]
    async fn disconnected_quiet_automaton_releases_connection_slot() {
        let slots = Arc::new(Semaphore::new(1));
        let permit = try_acquire_ws_slot(&slots).unwrap();
        let (events, live) = broadcast::channel(4);
        let (disconnect, disconnected) = oneshot::channel::<()>();
        let reader = tokio::spawn(async move {
            let _ = disconnected.await;
        });
        let attach = tokio::spawn(async move {
            let _permit = permit;
            let mut sink = futures_util::sink::drain();
            forward_automaton_subscription(
                &mut sink,
                EventSubscription {
                    history: Vec::new(),
                    live,
                    already_done: false,
                },
                reader,
            )
            .await;
        });
        tokio::task::yield_now().await;
        assert!(try_acquire_ws_slot(&slots).is_none());
        disconnect.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), attach)
            .await
            .expect("disconnect must release quiet attach without a new run event")
            .unwrap();
        assert!(try_acquire_ws_slot(&slots).is_some());
        // Run is still alive and was never stopped to recover the slot.
        assert_eq!(events.receiver_count(), 0);
    }

    #[tokio::test]
    async fn reader_disconnect_interrupts_blocked_history_write() {
        let (_events, live) = broadcast::channel(4);
        let (disconnect, disconnected) = oneshot::channel::<()>();
        let reader = tokio::spawn(async move {
            let _ = disconnected.await;
        });
        let attach = tokio::spawn(async move {
            let sink = futures_util::sink::unfold((), |(), _: axum::extract::ws::Message| async {
                std::future::pending::<Result<(), std::convert::Infallible>>().await
            });
            let mut sink = Box::pin(sink);
            forward_automaton_subscription(
                &mut sink,
                EventSubscription {
                    history: vec![aura_surface_automaton::AutomatonEvent::Done],
                    live,
                    already_done: true,
                },
                reader,
            )
            .await;
        });
        tokio::task::yield_now().await;
        assert!(!attach.is_finished());
        disconnect.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), attach)
            .await
            .expect("disconnect must interrupt stalled replay")
            .unwrap();
    }
}
