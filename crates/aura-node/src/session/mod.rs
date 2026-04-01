//! WebSocket session state and lifecycle.
//!
//! Each WebSocket connection maps to a `Session` that maintains conversation
//! state, tool configuration, and token accounting across turns.

mod helpers;
mod ws_handler;

pub use ws_handler::handle_ws_connection;

use crate::protocol::{self, SessionInit};
use aura_agent::{prompts::default_system_prompt, AgentLoopConfig};
use aura_core::{AgentId, InstalledToolDefinition};
use aura_reasoner::{Message, ModelProvider, ToolDefinition};
use aura_store::Store;
use aura_tools::automaton_tools::AutomatonController;
use aura_tools::domain_tools::DomainApi;
use aura_tools::{ToolCatalog, ToolConfig};
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
    /// Cumulative cache creation input tokens across all turns.
    pub cumulative_cache_creation_input_tokens: u64,
    /// Cumulative cache read input tokens across all turns.
    pub cumulative_cache_read_input_tokens: u64,
    /// Workspace directory for this session (sandboxed fallback).
    pub workspace: PathBuf,
    /// Base directory that workspace must reside under.
    workspace_base: PathBuf,
    /// Real project directory on the host filesystem.
    /// When set, tool execution uses this path directly.
    pub project_path: Option<PathBuf>,
    /// Optional base directory that project_path must reside under (remote VM mode).
    pub(super) project_base: Option<PathBuf>,
    /// Whether `session_init` has been received.
    pub initialized: bool,
    /// Available tool definitions (builtin + external).
    pub tool_definitions: Vec<ToolDefinition>,
    /// Context window size in tokens (for utilization calculation).
    pub context_window_tokens: u64,
    /// JWT auth token for proxy routing.
    pub auth_token: Option<String>,
    /// Project ID for domain tool calls.
    pub project_id: Option<String>,
    /// Project-agent UUID for X-Aura-Agent-Id billing header.
    pub aura_agent_id: Option<String>,
    /// Storage session UUID for X-Aura-Session-Id billing header.
    pub aura_session_id: Option<String>,
    /// Org UUID for X-Aura-Org-Id billing header.
    pub aura_org_id: Option<String>,
}

impl Session {
    /// Create a new uninitialized session with defaults.
    pub(super) fn new(default_workspace: PathBuf) -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            agent_id: AgentId::generate(),
            system_prompt: String::new(),
            model: aura_agent::DEFAULT_MODEL.to_string(),
            max_tokens: 16384,
            temperature: None,
            max_turns: 25,
            installed_tools: Vec::new(),
            messages: Vec::new(),
            cumulative_input_tokens: 0,
            cumulative_output_tokens: 0,
            cumulative_cache_creation_input_tokens: 0,
            cumulative_cache_read_input_tokens: 0,
            workspace: default_workspace.clone(),
            workspace_base: default_workspace,
            project_path: None,
            project_base: None,
            initialized: false,
            tool_definitions: Vec::new(),
            context_window_tokens: 200_000,
            auth_token: None,
            project_id: None,
            aura_agent_id: None,
            aura_session_id: None,
            aura_org_id: None,
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
            self.installed_tools = tools
                .into_iter()
                .map(protocol::installed_tool_to_core)
                .collect();
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
        if let Some(ref pp) = init.project_path {
            let candidate = PathBuf::from(pp);
            if !candidate.is_absolute() {
                return Err("project_path must be an absolute path".into());
            }
            if candidate
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err("project_path must not contain '..' components".into());
            }
            // When project_base is configured (remote VM mode), validate that
            // the project path is under it to prevent sandbox escape.
            if let Some(ref base) = self.project_base {
                let normalized = lexical_normalize(&candidate);
                let normalized_base = lexical_normalize(base);
                if !normalized.starts_with(&normalized_base) {
                    return Err(format!(
                        "project_path must be under {}",
                        base.display()
                    ));
                }
            }
            self.project_path = Some(candidate);
        }
        if let Some(token) = init.token {
            self.auth_token = Some(token);
        }
        if let Some(pid) = init.project_id {
            self.project_id = Some(pid);
        }
        if let Some(id) = init.aura_agent_id {
            self.aura_agent_id = Some(id);
        }
        if let Some(id) = init.aura_session_id {
            self.aura_session_id = Some(id);
        }
        if let Some(id) = init.aura_org_id {
            self.aura_org_id = Some(id);
        }
        if let Some(msgs) = init.conversation_messages {
            for msg in msgs {
                match msg.role.as_str() {
                    "user" => self.messages.push(Message::user(&msg.content)),
                    "assistant" => self.messages.push(Message::assistant(&msg.content)),
                    _ => {}
                }
            }
        }
        self.initialized = true;
        Ok(())
    }

    /// Build an `AgentLoopConfig` from session state.
    pub(super) fn agent_loop_config(&self) -> AgentLoopConfig {
        let base_prompt = if self.system_prompt.is_empty() {
            default_system_prompt()
        } else {
            self.system_prompt.clone()
        };

        let system_prompt = if let Some(ref pp) = self.project_path {
            format!(
                "{base_prompt}\n\n## Workspace\n\n\
                 Your workspace root is `{}`. All relative file paths are resolved against this directory. \
                 When referring to files, use paths relative to this root.",
                pp.display()
            )
        } else {
            base_prompt
        };

        AgentLoopConfig {
            max_iterations: self.max_turns as usize,
            model: self.model.clone(),
            system_prompt,
            max_tokens: self.max_tokens,
            max_context_tokens: Some(self.context_window_tokens),
            auth_token: self.auth_token.clone(),
            aura_project_id: self.project_id.clone(),
            aura_agent_id: self.aura_agent_id.clone(),
            aura_session_id: self.aura_session_id.clone(),
            aura_org_id: self.aura_org_id.clone(),
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
    /// Persistent store for kernel recording.
    pub store: Arc<dyn Store>,
    /// Tool configuration (fs/cmd permissions).
    pub tool_config: ToolConfig,
    /// JWT auth token from the WebSocket upgrade request.
    pub auth_token: Option<String>,
    /// Canonical tool catalog (shared across sessions).
    pub catalog: Arc<ToolCatalog>,
    /// Domain API for native spec/task/project/orbit/network tool execution.
    pub domain_api: Option<Arc<dyn DomainApi>>,
    /// Automaton controller for dev-loop lifecycle (None when domain API unavailable).
    pub automaton_controller: Option<Arc<dyn AutomatonController>>,
    /// Optional project base for remapping project paths (from `AURA_PROJECT_BASE`).
    pub project_base: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_protocol::SessionInit;

    fn test_session(project_base: Option<&str>) -> Session {
        let tmp = std::env::temp_dir().join("aura-test-session");
        let _ = std::fs::create_dir_all(&tmp);
        let mut s = Session::new(tmp);
        s.project_base = project_base.map(PathBuf::from);
        s
    }

    fn init_with_project_path(path: &str) -> SessionInit {
        SessionInit {
            system_prompt: None,
            model: None,
            max_tokens: None,
            temperature: None,
            max_turns: None,
            installed_tools: None,
            workspace: None,
            project_path: Some(path.into()),
            token: None,
            project_id: None,
            conversation_messages: None,
            aura_agent_id: None,
            aura_session_id: None,
            aura_org_id: None,
        }
    }

    #[test]
    fn project_path_allowed_when_no_base() {
        let mut session = test_session(None);
        let init = init_with_project_path("/any/absolute/path");
        assert!(session.apply_init(init).is_ok());
        assert_eq!(session.project_path.unwrap().to_str().unwrap(), "/any/absolute/path");
    }

    #[test]
    fn project_path_allowed_under_base() {
        let mut session = test_session(Some("/home/aura"));
        let init = init_with_project_path("/home/aura/myproject");
        assert!(session.apply_init(init).is_ok());
    }

    #[test]
    fn project_path_blocked_outside_base() {
        let mut session = test_session(Some("/home/aura"));
        let init = init_with_project_path("/etc/passwd");
        let result = session.apply_init(init);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be under"));
    }

    #[test]
    fn project_path_blocked_with_traversal() {
        let mut session = test_session(Some("/home/aura"));
        let init = init_with_project_path("/home/aura/../etc/passwd");
        let result = session.apply_init(init);
        assert!(result.is_err());
    }

    #[test]
    fn project_path_rejects_relative() {
        let mut session = test_session(None);
        let init = init_with_project_path("relative/path");
        let result = session.apply_init(init);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("absolute"));
    }
}
