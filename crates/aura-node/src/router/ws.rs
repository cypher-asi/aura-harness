use super::*;

/// Upgrade an HTTP connection to a WebSocket for interactive agent sessions.
pub(super) async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<RouterState>,
) -> impl IntoResponse {
    let auth_token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(String::from);

    let ctx = WsContext {
        workspace_base: state.config.workspaces_path(),
        provider: state.provider.clone(),
        store: state.store.clone(),
        tool_config: state.tool_config.clone(),
        auth_token,
        catalog: state.catalog.clone(),
        domain_api: state.domain_api.clone(),
        automaton_controller: state.automaton_controller.clone(),
        project_base: state.config.project_base.clone(),
    };
    ws.on_upgrade(move |socket| handle_ws_connection(socket, ctx))
}

/// WebSocket endpoint for streaming automaton events.
///
/// Clients connect to `/stream/automaton/:automaton_id` to receive real-time
/// events from a running automaton (dev loop, task run, etc.).
/// Requires a Bearer token in the Authorization header (same as the chat WS).
pub(super) async fn automaton_ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    Path(automaton_id): Path<String>,
    State(state): State<RouterState>,
) -> impl IntoResponse {
    let _auth_token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(String::from);

    ws.on_upgrade(move |socket| handle_automaton_ws(socket, automaton_id, state.automaton_bridge))
}

async fn handle_automaton_ws(
    socket: axum::extract::ws::WebSocket,
    automaton_id: String,
    bridge: Option<Arc<AutomatonBridge>>,
) {
    use axum::extract::ws::Message as WsMessage;
    use futures_util::{SinkExt, StreamExt};

    let (mut ws_tx, mut ws_rx) = socket.split();

    let bridge = match bridge {
        Some(b) => b,
        None => {
            let msg =
                serde_json::json!({"type": "error", "message": "automaton controller unavailable"})
                    .to_string();
            let _: Result<(), _> = ws_tx.send(WsMessage::Text(msg)).await;
            return;
        }
    };

    let mut rx = match bridge.subscribe_events(&automaton_id) {
        Some(rx) => rx,
        None => {
            let msg = serde_json::json!({"type": "error", "message": format!("automaton {automaton_id} not found or already finished")}).to_string();
            let _: Result<(), _> = ws_tx.send(WsMessage::Text(msg)).await;
            return;
        }
    };

    info!(automaton_id = %automaton_id, "Automaton event stream connected");

    // Drain the read side so the WebSocket layer can process ping/pong
    // and close frames. Without this the connection may be dropped by
    // intermediaries that expect pong responses.
    let drain_aid = automaton_id.clone();
    let drain_handle = tokio::spawn(async move {
        while let Some(msg) = ws_rx.next().await {
            match msg {
                Ok(WsMessage::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
        tracing::debug!(automaton_id = %drain_aid, "Automaton WS read side closed");
    });

    loop {
        match rx.recv().await {
            Ok(event) => {
                let is_done = matches!(event, aura_automaton::AutomatonEvent::Done);
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

    drain_handle.abort();
    info!(automaton_id = %automaton_id, "Automaton event stream disconnected");
}
