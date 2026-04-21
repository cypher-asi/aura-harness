use super::*;
use axum::response::Response;

/// Maximum size of a single WebSocket frame. Hardens against memory
/// amplification from oversized binary frames. 64 KiB matches typical
/// browser defaults. (Wave 5 / T1.3.)
pub(super) const WS_MAX_FRAME_BYTES: usize = 64 * 1024;

/// Maximum size of a reassembled WebSocket message (all frames combined).
/// Caps pathological large-message attacks while staying well above the
/// largest legitimate session payload we expect. (Wave 5 / T1.3.)
pub(super) const WS_MAX_MESSAGE_BYTES: usize = 256 * 1024;

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
        memory_manager: state.memory_manager.clone(),
        skill_manager: state.skill_manager.clone(),
        router_url: state.router_url.clone(),
    };
    ws.max_frame_size(WS_MAX_FRAME_BYTES)
        .max_message_size(WS_MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| handle_ws_connection(socket, ctx))
}

/// WebSocket endpoint for streaming automaton events.
///
/// Clients connect to `/stream/automaton/:automaton_id` to receive real-time
/// events from a running automaton (dev loop, task run, etc.). Requires a
/// non-empty Bearer token in the Authorization header — the prior
/// implementation parsed the token and then dropped it, which was
/// effectively anonymous. (Wave 5 / T1.4.)
pub(super) async fn automaton_ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    Path(automaton_id): Path<String>,
    State(state): State<RouterState>,
) -> Response {
    // Belt-and-suspenders: the router-wide `require_bearer_mw` middleware
    // (see `router::create_router`) has already rejected callers without
    // a valid Bearer header by the time we get here. Keeping the inline
    // check guards against accidental regressions — e.g. someone wiring
    // this handler up to a fresh `Router` that doesn't inherit the
    // middleware layer. Cost is a single `HeaderMap::get` on an already
    // authenticated path.
    if let Err(status) = super::auth::require_bearer(&headers, &state.config.auth_token) {
        return status.into_response();
    }

    ws.max_frame_size(WS_MAX_FRAME_BYTES)
        .max_message_size(WS_MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| handle_automaton_ws(socket, automaton_id, state.automaton_bridge))
        .into_response()
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
