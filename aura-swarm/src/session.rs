//! WebSocket session state and lifecycle.
//!
//! Each WebSocket connection maps to a `Session` that maintains conversation
//! state, tool configuration, and token accounting across turns.

use crate::protocol::*;
use aura_core::ExternalToolDefinition;
use aura_reasoner::{Message, ToolDefinition};
use aura_tools::ToolRegistry;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures_util::{SinkExt, StreamExt};
use std::path::PathBuf;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// ============================================================================
// Session
// ============================================================================

/// Per-connection session state.
pub struct Session {
    /// Unique session identifier.
    pub session_id: String,
    /// System prompt for the model.
    pub system_prompt: String,
    /// Model identifier.
    pub model: String,
    /// Max tokens per response.
    pub max_tokens: u32,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Maximum agentic steps per turn.
    pub max_turns: u32,
    /// External tools registered for this session.
    pub external_tools: Vec<ExternalToolDefinition>,
    /// Conversation history (accumulated across turns).
    pub messages: Vec<Message>,
    /// Cumulative input tokens across all turns.
    pub cumulative_input_tokens: u64,
    /// Cumulative output tokens across all turns.
    pub cumulative_output_tokens: u64,
    /// Workspace directory for this session.
    pub workspace: PathBuf,
    /// Whether session_init has been received.
    pub initialized: bool,
    /// Available tool definitions (builtin + external).
    pub tool_definitions: Vec<ToolDefinition>,
}

impl Session {
    /// Create a new uninitialized session with defaults.
    fn new(default_workspace: PathBuf) -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            system_prompt: String::new(),
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 16384,
            temperature: None,
            max_turns: 25,
            external_tools: Vec::new(),
            messages: Vec::new(),
            cumulative_input_tokens: 0,
            cumulative_output_tokens: 0,
            workspace: default_workspace,
            initialized: false,
            tool_definitions: Vec::new(),
        }
    }

    /// Apply a `session_init` message to configure this session.
    fn apply_init(&mut self, init: SessionInit) {
        if let Some(prompt) = init.system_prompt {
            self.system_prompt = prompt;
        }
        if let Some(model) = init.model {
            self.model = model;
        }
        if let Some(max_tokens) = init.max_tokens {
            self.max_tokens = max_tokens;
        }
        if let Some(temperature) = init.temperature {
            self.temperature = Some(temperature);
        }
        if let Some(max_turns) = init.max_turns {
            self.max_turns = max_turns;
        }
        if let Some(tools) = init.external_tools {
            self.external_tools = tools;
        }
        if let Some(workspace) = init.workspace {
            self.workspace = PathBuf::from(workspace);
        }
        self.initialized = true;
    }
}

// ============================================================================
// WebSocket Handler
// ============================================================================

/// Configuration passed to the WebSocket handler from the router state.
#[derive(Clone)]
pub struct WsContext {
    /// Default workspace base path.
    pub workspace_base: PathBuf,
}

/// Handle a WebSocket connection through its full lifecycle.
///
/// Protocol:
/// 1. Client sends `session_init` as the first message.
/// 2. Server responds with `session_ready`.
/// 3. Client sends `user_message` events, server streams responses.
pub async fn handle_ws_connection(socket: WebSocket, ctx: WsContext) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<OutboundMessage>();

    // Spawn a task that forwards outbound messages to the WebSocket sink.
    let send_task = tokio::spawn(async move {
        while let Some(msg) = outbound_rx.recv().await {
            match serde_json::to_string(&msg) {
                Ok(json) => {
                    if ws_tx.send(WsMessage::Text(json.into())).await.is_err() {
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
    info!(session_id = %session.session_id, "WebSocket connection opened");

    // Message receive loop.
    while let Some(msg_result) = ws_rx.next().await {
        let raw = match msg_result {
            Ok(WsMessage::Text(text)) => text.to_string(),
            Ok(WsMessage::Close(_)) => {
                debug!(session_id = %session.session_id, "Client sent close frame");
                break;
            }
            Ok(WsMessage::Ping(_) | WsMessage::Pong(_)) => continue,
            Ok(_) => continue,
            Err(e) => {
                warn!(session_id = %session.session_id, error = %e, "WebSocket receive error");
                break;
            }
        };

        let inbound: InboundMessage = match serde_json::from_str(&raw) {
            Ok(msg) => msg,
            Err(e) => {
                let _ = outbound_tx.send(OutboundMessage::Error(ErrorMsg {
                    code: "parse_error".into(),
                    message: format!("Invalid message: {e}"),
                    recoverable: true,
                }));
                continue;
            }
        };

        match inbound {
            InboundMessage::SessionInit(init) => {
                handle_session_init(&mut session, init, &outbound_tx);
            }
            InboundMessage::UserMessage(msg) => {
                handle_user_message(&mut session, msg, &outbound_tx).await;
            }
            InboundMessage::Cancel => {
                debug!(session_id = %session.session_id, "Cancel requested (not yet implemented)");
            }
            InboundMessage::ApprovalResponse(resp) => {
                debug!(
                    session_id = %session.session_id,
                    tool_use_id = %resp.tool_use_id,
                    approved = resp.approved,
                    "Approval response received (not yet implemented)"
                );
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

    // Build tool list from builtins (external tools added in Phase 2b).
    let builtin_tools = aura_tools::DefaultToolRegistry::new();
    session.tool_definitions = builtin_tools.list();

    // Add external tool definitions.
    for ext in &session.external_tools {
        session.tool_definitions.push(ToolDefinition::new(
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

/// Handle a `user_message` — stub that sends a placeholder error.
///
/// Full turn processing is wired in Phase 2b/2c.
async fn handle_user_message(
    session: &mut Session,
    msg: UserMessage,
    outbound_tx: &mpsc::UnboundedSender<OutboundMessage>,
) {
    if !session.initialized {
        let _ = outbound_tx.send(OutboundMessage::Error(ErrorMsg {
            code: "not_initialized".into(),
            message: "Send session_init before user_message".into(),
            recoverable: true,
        }));
        return;
    }

    debug!(
        session_id = %session.session_id,
        content_len = msg.content.len(),
        "Received user_message (turn processing not yet wired)"
    );

    let _ = outbound_tx.send(OutboundMessage::Error(ErrorMsg {
        code: "not_implemented".into(),
        message: "Turn processing will be connected in Phase 2b/2c".into(),
        recoverable: true,
    }));
}
