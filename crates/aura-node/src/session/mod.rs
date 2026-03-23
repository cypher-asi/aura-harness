//! WebSocket session state and lifecycle.
//!
//! Each WebSocket connection maps to a `Session` that maintains conversation
//! state, tool configuration, and token accounting across turns.

mod ws_handler;

pub use ws_handler::handle_ws_connection;

use crate::protocol::SessionInit;
use aura_agent::{prompts::default_system_prompt, AgentLoopConfig};
use aura_core::{AgentId, InstalledToolDefinition};
use aura_reasoner::{Message, ModelProvider, ToolDefinition};
use aura_tools::ToolConfig;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

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
    /// Installed tools registered for this session.
    pub installed_tools: Vec<InstalledToolDefinition>,
    /// Conversation history (accumulated across turns).
    pub messages: Vec<Message>,
    /// Cumulative input tokens across all turns.
    pub cumulative_input_tokens: u64,
    /// Cumulative output tokens across all turns.
    pub cumulative_output_tokens: u64,
    /// Workspace directory for this session.
    pub workspace: PathBuf,
    /// Base directory that workspace must reside under.
    workspace_base: PathBuf,
    /// Whether `session_init` has been received.
    pub initialized: bool,
    /// Available tool definitions (builtin + external).
    pub tool_definitions: Vec<ToolDefinition>,
    /// Context window size in tokens (for utilization calculation).
    pub context_window_tokens: u64,
    /// JWT auth token for proxy routing.
    pub auth_token: Option<String>,
}

impl Session {
    /// Create a new uninitialized session with defaults.
    pub(super) fn new(default_workspace: PathBuf) -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            agent_id: AgentId::generate(),
            system_prompt: String::new(),
            model: "claude-opus-4-6".to_string(),
            max_tokens: 16384,
            temperature: None,
            max_turns: 25,
            installed_tools: Vec::new(),
            messages: Vec::new(),
            cumulative_input_tokens: 0,
            cumulative_output_tokens: 0,
            workspace: default_workspace.clone(),
            workspace_base: default_workspace,
            initialized: false,
            tool_definitions: Vec::new(),
            context_window_tokens: 200_000,
            auth_token: None,
        }
    }

    /// Apply a `session_init` message to configure this session.
    pub(super) fn apply_init(&mut self, init: SessionInit) -> Result<(), String> {
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
        if let Some(tools) = init.installed_tools {
            self.installed_tools = tools;
        }
        if let Some(workspace) = init.workspace {
            let candidate = PathBuf::from(&workspace);
            if candidate
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err("workspace path must not contain '..' components".into());
            }
            let normalized = lexical_normalize(&candidate);
            let normalized_base = lexical_normalize(&self.workspace_base);
            if !normalized.starts_with(&normalized_base) {
                return Err(format!(
                    "workspace path must be under {}",
                    self.workspace_base.display()
                ));
            }
            self.workspace = candidate;
        }
        if let Some(token) = init.token {
            self.auth_token = Some(token);
        }
        self.initialized = true;
        Ok(())
    }

    /// Build an `AgentLoopConfig` from session state.
    pub(super) fn agent_loop_config(&self) -> AgentLoopConfig {
        AgentLoopConfig {
            max_iterations: self.max_turns as usize,
            model: self.model.clone(),
            system_prompt: if self.system_prompt.is_empty() {
                default_system_prompt()
            } else {
                self.system_prompt.clone()
            },
            max_tokens: self.max_tokens,
            max_context_tokens: Some(self.context_window_tokens),
            auth_token: self.auth_token.clone(),
            ..AgentLoopConfig::default()
        }
    }
}

fn lexical_normalize(path: &std::path::Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

// ============================================================================
// WebSocket Handler Context
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
    /// JWT auth token from the WebSocket upgrade request.
    pub auth_token: Option<String>,
}
