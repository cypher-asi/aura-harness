//! WebSocket session state and lifecycle.
//!
//! Each WebSocket connection maps to a `Session` that maintains conversation
//! state, tool configuration, and token accounting across turns.

use crate::protocol::*;
use aura_core::{AgentId, ExternalToolDefinition};
use aura_executor::ExecutorRouter;
use aura_kernel::{StreamCallback, StreamCallbackEvent, TurnConfig, TurnProcessor};
use aura_reasoner::{
    Message, ModelProvider, ModelRequest, ModelResponse, StreamEventStream, ToolDefinition,
};
use aura_store::{AgentStatus, Store, StoreError};
use aura_tools::{DefaultToolRegistry, ToolConfig, ToolExecutor, ToolRegistry};
use async_trait::async_trait;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures_util::{SinkExt, StreamExt};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// ============================================================================
// DynProvider — type-erased ModelProvider wrapper
// ============================================================================

/// Wraps `Arc<dyn ModelProvider + Send + Sync>` so it can be used as
/// a concrete `P` type parameter in `TurnProcessor<P, S, R>`.
pub(crate) struct DynProvider(pub Arc<dyn ModelProvider + Send + Sync>);

#[async_trait]
impl ModelProvider for DynProvider {
    fn name(&self) -> &'static str {
        self.0.name()
    }

    async fn complete(&self, request: ModelRequest) -> anyhow::Result<ModelResponse> {
        self.0.complete(request).await
    }

    async fn complete_streaming(&self, request: ModelRequest) -> anyhow::Result<StreamEventStream> {
        self.0.complete_streaming(request).await
    }

    async fn health_check(&self) -> bool {
        self.0.health_check().await
    }
}

// ============================================================================
// NullStore — lightweight store for WebSocket sessions
// ============================================================================

/// Minimal `Store` implementation for WebSocket sessions that manage
/// their own message history and don't need persistent storage.
pub(crate) struct NullStore;

impl Store for NullStore {
    fn enqueue_tx(&self, _tx: &aura_core::Transaction) -> Result<(), StoreError> {
        Ok(())
    }

    fn dequeue_tx(
        &self,
        _agent_id: AgentId,
    ) -> Result<Option<(u64, aura_core::Transaction)>, StoreError> {
        Ok(None)
    }

    fn get_head_seq(&self, _agent_id: AgentId) -> Result<u64, StoreError> {
        Ok(0)
    }

    fn append_entry_atomic(
        &self,
        _agent_id: AgentId,
        _next_seq: u64,
        _entry: &aura_core::RecordEntry,
        _dequeued_inbox_seq: u64,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    fn scan_record(
        &self,
        _agent_id: AgentId,
        _from_seq: u64,
        _limit: usize,
    ) -> Result<Vec<aura_core::RecordEntry>, StoreError> {
        Ok(Vec::new())
    }

    fn get_record_entry(
        &self,
        agent_id: AgentId,
        seq: u64,
    ) -> Result<aura_core::RecordEntry, StoreError> {
        Err(StoreError::RecordEntryNotFound(agent_id, seq))
    }

    fn get_agent_status(&self, _agent_id: AgentId) -> Result<AgentStatus, StoreError> {
        Ok(AgentStatus::Active)
    }

    fn set_agent_status(
        &self,
        _agent_id: AgentId,
        _status: AgentStatus,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    fn has_pending_tx(&self, _agent_id: AgentId) -> Result<bool, StoreError> {
        Ok(false)
    }

    fn get_inbox_depth(&self, _agent_id: AgentId) -> Result<u64, StoreError> {
        Ok(0)
    }
}

// ============================================================================
// Session
// ============================================================================

/// Per-connection session state.
pub struct Session {
    /// Unique session identifier.
    pub session_id: String,
    /// Stable agent ID for the lifetime of this session.
    pub agent_id: AgentId,
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
            agent_id: AgentId::generate(),
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

    /// Build a `TurnConfig` from session state.
    fn turn_config(&self) -> TurnConfig {
        TurnConfig {
            max_steps: self.max_turns,
            model: self.model.clone(),
            system_prompt: if self.system_prompt.is_empty() {
                TurnConfig::default().system_prompt
            } else {
                self.system_prompt.clone()
            },
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            workspace_base: self.workspace.clone(),
            ..TurnConfig::default()
        }
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
    /// Shared model provider (type-erased).
    pub provider: Arc<dyn ModelProvider + Send + Sync>,
    /// Tool configuration (fs/cmd permissions).
    pub tool_config: ToolConfig,
}

/// Handle a WebSocket connection through its full lifecycle.
///
/// Protocol:
/// 1. Client sends `session_init` as the first message.
/// 2. Server responds with `session_ready`.
/// 3. Client sends `user_message` events, server streams responses.
/// 4. Message history accumulates across turns for multi-turn conversation.
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
                handle_user_message(&mut session, msg, &outbound_tx, &ctx).await;
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

    // Build tool list from builtins.
    let builtin_tools = DefaultToolRegistry::new();
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

/// Handle a `user_message` by running the agentic turn loop via `TurnProcessor`.
///
/// The session's `messages` vec is grown in-place across turns: the user message
/// is appended before the turn, and the assistant's messages (text + tool use +
/// tool results) are appended by the turn loop. On the next `user_message`,
/// the full history is sent again, giving the model multi-turn context.
async fn handle_user_message(
    session: &mut Session,
    msg: UserMessage,
    outbound_tx: &mpsc::UnboundedSender<OutboundMessage>,
    ctx: &WsContext,
) {
    if !session.initialized {
        let _ = outbound_tx.send(OutboundMessage::Error(ErrorMsg {
            code: "not_initialized".into(),
            message: "Send session_init before user_message".into(),
            recoverable: true,
        }));
        return;
    }

    let message_id = Uuid::new_v4().to_string();
    let _ = outbound_tx.send(OutboundMessage::AssistantMessageStart(
        AssistantMessageStart {
            message_id: message_id.clone(),
        },
    ));

    // Append the user message to the conversation history.
    session.messages.push(Message::user(&msg.content));

    // Build per-turn tool executor with external tools.
    let mut tool_executor = ToolExecutor::new(ctx.tool_config.clone());
    for ext in &session.external_tools {
        tool_executor.register_external(aura_tools::ExternalToolDefinition {
            name: ext.name.clone(),
            description: ext.description.clone(),
            input_schema: ext.input_schema.clone(),
            callback_url: ext.callback_url.clone(),
        });
    }
    let mut executor_router = ExecutorRouter::new();
    executor_router.add_executor(Arc::new(tool_executor));

    // Build per-turn tool registry with external tools.
    let mut tool_registry = DefaultToolRegistry::new();
    for ext in &session.external_tools {
        tool_registry.register(ToolDefinition::new(
            &ext.name,
            &ext.description,
            ext.input_schema.clone(),
        ));
    }

    // Set up streaming callback → channel → WebSocket bridge.
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<StreamCallbackEvent>();
    let outbound_for_stream = outbound_tx.clone();

    let stream_forward_task = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let out = match event {
                StreamCallbackEvent::TextDelta(text) => {
                    OutboundMessage::TextDelta(TextDelta { text })
                }
                StreamCallbackEvent::ThinkingDelta(thinking) => {
                    OutboundMessage::ThinkingDelta(ThinkingDelta { thinking })
                }
                StreamCallbackEvent::ThinkingComplete => continue,
                StreamCallbackEvent::ToolStart { id, name } => {
                    OutboundMessage::ToolUseStart(ToolUseStart { id, name })
                }
                StreamCallbackEvent::ToolComplete {
                    name,
                    result,
                    is_error,
                    ..
                } => OutboundMessage::ToolResult(ToolResultMsg {
                    name,
                    result,
                    is_error,
                }),
                StreamCallbackEvent::StepComplete => continue,
            };
            if outbound_for_stream.send(out).is_err() {
                break;
            }
        }
    });

    let callback: StreamCallback = Box::new(move |event| {
        let _ = event_tx.send(event);
    });

    // Create a TurnProcessor for this turn.
    let provider = Arc::new(DynProvider(ctx.provider.clone()));
    let store = Arc::new(NullStore);
    let turn_config = session.turn_config();

    let mut processor = TurnProcessor::new(
        provider,
        store,
        executor_router,
        Arc::new(tool_registry),
        turn_config,
    );
    processor.set_stream_callback(Arc::new(callback));

    // Snapshot the messages for the turn. The processor will extend them
    // internally (adding assistant messages + tool results), and we'll
    // capture the final state afterwards.
    let messages_for_turn = session.messages.clone();

    let result = processor
        .process_turn_with_messages(session.agent_id, messages_for_turn)
        .await;

    // Drop the processor (and its callback) so the event_tx sender is freed,
    // which allows the stream_forward_task to complete.
    drop(processor);
    let _ = stream_forward_task.await;

    match result {
        Ok(turn_result) => {
            // Reconstruct the messages that the turn loop added (assistant
            // messages and tool results) so the session history stays in sync.
            for entry in &turn_result.entries {
                session.messages.push(entry.model_response.message.clone());

                let tool_results = &entry.tool_results;
                if !tool_results.is_empty() {
                    session
                        .messages
                        .push(Message::tool_results(tool_results.clone()));
                }
            }

            let input_tokens = u64::from(turn_result.total_input_tokens);
            let output_tokens = u64::from(turn_result.total_output_tokens);
            session.cumulative_input_tokens += input_tokens;
            session.cumulative_output_tokens += output_tokens;

            let stop_reason = if turn_result.had_failures {
                "end_turn_with_errors"
            } else {
                "end_turn"
            };

            let _ = outbound_tx.send(OutboundMessage::AssistantMessageEnd(
                AssistantMessageEnd {
                    message_id,
                    stop_reason: stop_reason.into(),
                    usage: SessionUsage {
                        input_tokens,
                        output_tokens,
                        cumulative_input_tokens: session.cumulative_input_tokens,
                        cumulative_output_tokens: session.cumulative_output_tokens,
                    },
                },
            ));

            info!(
                session_id = %session.session_id,
                history_len = session.messages.len(),
                cumulative_in = session.cumulative_input_tokens,
                cumulative_out = session.cumulative_output_tokens,
                "Turn complete"
            );
        }
        Err(e) => {
            error!(session_id = %session.session_id, error = %e, "Turn processing failed");
            let _ = outbound_tx.send(OutboundMessage::Error(ErrorMsg {
                code: "turn_error".into(),
                message: format!("Turn processing failed: {e}"),
                recoverable: true,
            }));
        }
    }
}
