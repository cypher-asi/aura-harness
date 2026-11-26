//! Handlers mounted directly by the router: the terminal WebSocket upgrade
//! and `/health`.

use axum::{
    extract::{ws::WebSocketUpgrade, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use tracing::warn;

use super::super::handlers::run_ws;
use super::super::RouterState;
use crate::inbound_console;

// === Terminal WebSocket ===

pub(super) async fn terminal_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<RouterState>,
) -> axum::response::Response {
    let Some(permit) = run_ws::try_acquire_ws_slot(&state.ws_slots) else {
        warn!(
            cap = run_ws::MAX_WS_CONNS_PER_NODE,
            "Refusing /ws/terminal upgrade: WS connection cap reached"
        );
        inbound_console::ws_rejection_line(
            "upgrade.terminal",
            "slot_full",
            Some(&format!("cap={}", run_ws::MAX_WS_CONNS_PER_NODE)),
        );
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    ws.on_upgrade(move |socket| async move {
        // `permit` is moved into the per-socket task so the slot
        // is only released when the socket task actually exits.
        super::super::terminal_ws::handle_terminal_ws(socket).await;
        drop(permit);
    })
    .into_response()
}

// === Health ===

/// Return a liveness/readiness response with version + execution guardrails.
///
/// The tool-policy fields (`run_command_enabled`, `shell_enabled`,
/// `allowed_commands`, `binary_allowlist`) expose the effective executor config so
/// external consumers can diff the running harness's policy against
/// what they need. Historically the `aura-os-desktop` external-harness
/// probe relied on `run_command_enabled` to fail fast when the
/// operator forgot `AURA_AUTONOMOUS_DEV_LOOP=1`; the fields are now
/// diagnostics for the command execution guardrails, not catalog visibility.
///
/// The response is deliberately unauthenticated (matches the old
/// minimal-health behaviour) because the information is non-sensitive:
/// anyone who can already reach the health port can trivially discover
/// the same facts by sending any tool invocation and observing the
/// denial. Fields are additive — a missing field in older harness
/// versions means "unknown", and the desktop treats unknown as a warn
/// (not a hard-fail) so mixed-version fleets keep working.
pub(super) async fn health_handler(State(state): State<RouterState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        // Build-time git commit baked into the container image (see
        // Dockerfile ARG GIT_SHA). Absent for local/non-container builds.
        "git_sha": std::env::var("AURA_HARNESS_GIT_SHA").ok().filter(|s| !s.is_empty()),
        "run_command_enabled": state.tool_config.command.enabled,
        "shell_enabled": state.tool_config.command.allow_shell,
        "allowed_commands": state.tool_config.command.command_allowlist,
        "binary_allowlist": state.tool_config.command.binary_allowlist,
        // Additive capability negotiation for separately deployed Aura OS
        // servers. Older harnesses omit this field and therefore fail closed.
        "safe_workspace": true,
    }))
}
