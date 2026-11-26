//! Session state management.
//!
//! This file owns the [`Session`] struct plus everything that maintains
//! its per-connection state: `new`, `apply_chat_runtime_request`, the
//! wire→core permission translator, the intent-classifier builder,
//! `AgentLoopConfig` derivation, and the agent loop configuration
//! derived from session state.
//!
//! Phase A note: the chat session bootstrap used to be driven by an
//! `InboundMessage::SessionInit` first WS frame. With the new
//! `POST /v1/run` + `WS /stream/:run_id` flow the chat fields ship
//! on a [`aura_core_protocol::RuntimeRequest`] over HTTP and the session is fully
//! initialized before the WebSocket attaches.

mod apply;
mod wire;

pub(crate) use wire::{agent_permissions_from_wire, is_managed_safe_workspace_path};

use crate::gateway::session::ToolApprovalBroker;
use aura_agent::AgentLoopConfig;
use aura_context_prompts::default_system_prompt;
use aura_core_protocol::SessionModelOverrides;
use aura_core_types::{
    AgentId, AgentPermissions, AgentToolPermissions, InstalledIntegrationDefinition,
    InstalledToolDefinition,
};
use aura_engine::scheduler::AgentIdentity as RuntimeAgentIdentity;
use aura_exec_tools::IntentClassifier;
use aura_model::{Message, ModelProvider, ModelRequestKind, PromptCacheRetention, ToolDefinition};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

// ============================================================================
// Session
// ============================================================================

/// Per-connection session state.
///
/// Fields are `pub(crate)` so only the node crate may mutate them; external
/// crates must go through the public accessors / constructors we expose on
/// purpose.
pub struct Session {
    /// Unique session identifier.
    pub(crate) session_id: String,
    /// Stable agent ID for the lifetime of this session.
    pub(crate) agent_id: AgentId,
    /// System prompt for the model.
    pub(crate) system_prompt: String,
    /// Model identifier.
    pub(crate) model: String,
    /// Provider identifier for this session.
    pub(crate) provider_name: String,
    /// Optional per-session model overrides resolved from the runtime
    /// request.
    pub(crate) provider_overrides: Option<SessionModelOverrides>,
    /// Stable OpenAI-family `prompt_cache_key` resolved from the
    /// request's `provider_overrides` bundle.
    pub(crate) prompt_cache_key: Option<String>,
    /// Optional OpenAI-family `prompt_cache_retention` paired with
    /// `prompt_cache_key`.
    pub(crate) prompt_cache_retention: Option<String>,
    /// Optional concrete provider override built from
    /// `provider_overrides`.
    pub(crate) provider_override: Option<Arc<dyn ModelProvider + Send + Sync>>,
    /// Sampling temperature.
    pub(crate) temperature: Option<f32>,
    /// User-selected reasoning-effort tier forwarded from the chat
    /// model picker via `ModelSelection::reasoning_effort`. When set it
    /// hard-pins the agent loop's thinking effort (see
    /// `AgentLoopConfig::user_thinking_effort`).
    pub(crate) user_thinking_effort: Option<aura_model::ThinkingEffort>,
    /// Model that titles the chat from the next turn's prompt, from
    /// `ModelSelection::title_model`; taken by that turn.
    pub(crate) title_model: Option<String>,
    /// Maximum agentic steps per turn.
    pub(crate) max_turns: u32,
    /// Installed tools registered for this session.
    pub(crate) installed_tools: Vec<InstalledToolDefinition>,
    /// Installed integrations authorized for this session.
    pub(crate) installed_integrations: Vec<InstalledIntegrationDefinition>,
    /// Conversation history (accumulated across turns).
    pub(crate) messages: Vec<Message>,
    /// Token counts summed across all turns.
    pub(crate) cumulative: CumulativeTokens,
    /// Workspace directory for this session (sandboxed fallback).
    pub(crate) workspace: PathBuf,
    /// Base directory that workspace must reside under.
    pub(crate) workspace_base: PathBuf,
    /// Real project directory on the host filesystem.
    /// When set, tool execution uses this path directly.
    pub(crate) project_path: Option<PathBuf>,
    /// Optional base directory that project_path must reside under
    /// (remote VM mode).
    pub(super) project_base: Option<PathBuf>,
    /// Whether the chat runtime request has been applied.
    pub(crate) initialized: bool,
    /// Available tool definitions (builtin + external).
    pub(crate) tool_definitions: Vec<ToolDefinition>,
    /// Context window size in tokens (for utilization calculation).
    pub(crate) context_window_tokens: u64,
    /// JWT auth token for proxy routing.
    pub(crate) auth_token: Option<String>,
    /// Project ID for domain tool calls.
    pub(crate) project_id: Option<String>,
    /// Project-agent UUID for X-Aura-Agent-Id billing header.
    pub(crate) aura_agent_id: Option<String>,
    /// Storage session UUID for X-Aura-Session-Id billing header.
    pub(crate) aura_session_id: Option<String>,
    /// Org UUID for X-Aura-Org-Id billing header.
    pub(crate) aura_org_id: Option<String>,
    /// Harness-level agent ID for per-agent skill lookup.
    pub(crate) skill_agent_id: Option<String>,
    /// Optional keyword-driven intent classifier that narrows the
    /// visible tool set per turn.
    pub(crate) intent_classifier: Option<Arc<IntentClassifier>>,
    /// `(tool_name, domain)` pairs paired with [`intent_classifier`].
    ///
    /// [`intent_classifier`]: Self::intent_classifier
    pub(crate) intent_classifier_manifest: Vec<(String, String)>,
    /// Agent permissions for this session, derived directly from the
    /// required `RuntimeRequest.agent_permissions` field. Always
    /// applied to the kernel `PolicyConfig` on kernel construction.
    pub(crate) agent_permissions: AgentPermissions,
    /// Originating user id for tool-default resolution and forever
    /// approvals.
    pub(crate) user_id: String,
    /// Optional per-agent tool override for this session.
    pub(crate) tool_permissions: Option<AgentToolPermissions>,
    /// Live approval broker attached to this WebSocket connection.
    pub(crate) tool_approval_broker: Option<Arc<ToolApprovalBroker>>,
    /// Chat-WS migration: set when `apply_chat_runtime_request`
    /// assembled [`Self::system_prompt`] from the typed identity /
    /// project_info fields via [`SystemPromptBuilder`].
    pub(crate) typed_chat_prompt: bool,
    /// Whether this run opted into Anthropic computer-use. Drives both
    /// the `Capability::ComputerUse` injection (tool visibility) and the
    /// live `ComputerTool` registration on the session resolver.
    pub(crate) computer_use: bool,
    /// Base URL of the desktop computer-use executor the `ComputerTool`
    /// forwards actions to. `None` disables forwarding even when
    /// [`Self::computer_use`] is set.
    pub(crate) computer_executor_url: Option<String>,
    /// Session mode, resolved once from the request and the daemon
    /// default. The kernel policy denies any tool call it does not allow.
    pub(crate) mode: aura_core_types::AgentMode,
    /// Messages from agents this session spawned, waiting for an idle
    /// moment to run as a turn.
    pub(crate) agent_mailbox: Arc<super::agent_inbox::AgentMailbox>,
    /// Run trace of this conversation, started on its first turn.
    pub(crate) trace: Option<Arc<aura_agent_trace::RunRecorder>>,
    /// Where each turn's end is checkpointed; set on a cloud node, where
    /// the node holds the agent's conversation between runs.
    pub(crate) checkpoints: Option<aura_agent::checkpoint::Checkpoints>,
    /// Whether this run continues the conversation the agent's record
    /// already holds, so it starts no new session there.
    pub(crate) continues: bool,
    /// When the conversation started; names its checkpoint.
    pub(crate) started_ms: u64,
    /// Model calls and tokens of the conversation, across runs.
    pub(crate) usage: aura_agent::checkpoint::SessionUsage,
    /// The agent's todo list, carried from turn to turn.
    pub(crate) todos: Vec<aura_exec_tools::todo::TodoItem>,
}

/// A [`Session`]'s token counts summed across all its turns.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct CumulativeTokens {
    /// Cumulative input tokens across all turns.
    pub(crate) input_tokens: u64,
    /// Cumulative output tokens across all turns.
    pub(crate) output_tokens: u64,
    /// Cumulative cache creation input tokens across all turns.
    pub(crate) cache_creation_input_tokens: u64,
    /// Cumulative cache read input tokens across all turns.
    pub(crate) cache_read_input_tokens: u64,
}

impl Session {
    /// Create a new uninitialized session with defaults.
    pub(super) fn new(default_workspace: PathBuf) -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            agent_id: AgentId::generate(),
            system_prompt: String::new(),
            model: String::new(),
            provider_name: String::new(),
            provider_overrides: None,
            prompt_cache_key: None,
            prompt_cache_retention: None,
            provider_override: None,
            temperature: None,
            user_thinking_effort: None,
            title_model: None,
            max_turns: aura_core_types::MAX_TURNS,
            installed_tools: Vec::new(),
            installed_integrations: Vec::new(),
            messages: Vec::new(),
            cumulative: CumulativeTokens::default(),
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
            skill_agent_id: None,
            intent_classifier: None,
            intent_classifier_manifest: Vec::new(),
            agent_permissions: AgentPermissions::full_access(),
            user_id: String::new(),
            tool_permissions: None,
            tool_approval_broker: None,
            typed_chat_prompt: false,
            computer_use: false,
            computer_executor_url: None,
            mode: aura_core_types::AgentMode::Agent,
            agent_mailbox: Arc::default(),
            trace: None,
            checkpoints: None,
            continues: false,
            started_ms: aura_agent::checkpoint::now_ms(),
            usage: aura_agent::checkpoint::SessionUsage::default(),
            todos: Vec::new(),
        }
    }

    /// Return a deterministic `AgentId` for memory keying.
    ///
    /// When the session carries an `aura_agent_id` (the aura-os UUID),
    /// derive the `AgentId` from it so memory queries from the UI use
    /// the same key. Falls back to the random session `agent_id`.
    pub(super) fn memory_agent_id(&self) -> AgentId {
        if let Some(ref uuid_str) = self.aura_agent_id {
            if let Ok(uuid) = uuid::Uuid::parse_str(uuid_str) {
                return AgentId::from_uuid(uuid);
            }
        }
        self.agent_id
    }

    /// Snapshot the session's identity for the runtime-scheduler-side
    /// `AgentIdentityRegistry`.
    pub(crate) fn as_runtime_identity(&self) -> RuntimeAgentIdentity {
        let prompt_cache_retention = self
            .prompt_cache_retention
            .as_deref()
            .and_then(parse_session_cache_retention);
        RuntimeAgentIdentity {
            reasoning_effort: self.user_thinking_effort,
            upstream_provider_family: None,
            model: self.model.clone(),
            aura_org_id: self.aura_org_id.clone(),
            aura_session_id: self.aura_session_id.clone(),
            aura_agent_id: self.aura_agent_id.clone(),
            aura_project_id: self.project_id.clone(),
            system_prompt: self.resolved_system_prompt(),
            prompt_cache_key: self.prompt_cache_key.clone(),
            prompt_cache_retention,
            request_kind: ModelRequestKind::Chat,
            max_context_tokens: usize::try_from(self.context_window_tokens).unwrap_or(usize::MAX),
            auth_token: self.auth_token.clone(),
            cache_salt: None,
            run_scratch: None,
        }
    }

    /// Resolve the system prompt the same way [`Self::agent_loop_config`]
    /// does (typed-chat path skips the legacy `## Workspace` addendum).
    fn resolved_system_prompt(&self) -> String {
        let base_prompt = if self.system_prompt.is_empty() {
            default_system_prompt()
        } else {
            self.system_prompt.clone()
        };
        match (&self.project_path, self.typed_chat_prompt) {
            (Some(pp), false) => format!(
                "{base_prompt}\n\n## Workspace\n\n\
                 Your workspace root is `{}`. All relative file paths are resolved against this directory. \
                 When referring to files, use paths relative to this root.",
                pp.display()
            ),
            _ => base_prompt,
        }
    }

    /// Build an `AgentLoopConfig` from session state.
    pub(super) fn agent_loop_config(&self) -> AgentLoopConfig {
        let base_prompt = if self.system_prompt.is_empty() {
            default_system_prompt()
        } else {
            self.system_prompt.clone()
        };

        let system_prompt = match (&self.project_path, self.typed_chat_prompt) {
            (Some(pp), false) => format!(
                "{base_prompt}\n\n## Workspace\n\n\
                 Your workspace root is `{}`. All relative file paths are resolved against this directory. \
                 When referring to files, use paths relative to this root.",
                pp.display()
            ),
            _ => base_prompt,
        };

        let max_iterations = if self.max_turns == u32::MAX {
            usize::MAX
        } else {
            self.max_turns as usize
        };

        AgentLoopConfig {
            max_iterations,
            system_prompt,
            max_context_tokens: Some(self.context_window_tokens),
            stream_timeout: agent_loop_stream_timeout(),
            auth_token: self.auth_token.clone(),
            upstream_provider_family: None,
            // Chat picker's thinking-level selection hard-pins effort
            // for the whole turn; `None` keeps the internal taper.
            user_thinking_effort: self.user_thinking_effort,
            aura_project_id: self.project_id.clone(),
            aura_agent_id: self.aura_agent_id.clone(),
            aura_session_id: self.aura_session_id.clone(),
            aura_org_id: self.aura_org_id.clone(),
            intent_classifier: self.intent_classifier.clone(),
            intent_classifier_manifest: self.intent_classifier_manifest.clone(),
            prompt_cache_key: self.prompt_cache_key.clone(),
            prompt_cache_retention: self.prompt_cache_retention.clone(),
            initial_todos: self.todos.clone(),
            ..AgentLoopConfig::for_agent(self.model.clone())
        }
    }
}

/// Convert the wire-side `prompt_cache_retention` (`"24h"` /
/// `"in_memory"`) into the typed reasoner enum.
fn parse_session_cache_retention(value: &str) -> Option<PromptCacheRetention> {
    match value {
        "24h" => Some(PromptCacheRetention::Hours24),
        "in_memory" => Some(PromptCacheRetention::InMemory),
        _ => None,
    }
}

/// Reasoner reqwest HTTP timeout, in milliseconds, when
/// `AURA_MODEL_TIMEOUT_MS` is unset or unparsable.
const REASONER_DEFAULT_TIMEOUT_MS: u64 = 300_000;

/// Safety margin added on top of the reasoner's reqwest timeout when
/// computing the agent-loop outer-guard `stream_timeout`.
const STREAM_TIMEOUT_MARGIN_SECS: u64 = 30;

/// Outer-guard streaming timeout used by the chat-session
/// [`AgentLoopConfig`].
pub(crate) fn agent_loop_stream_timeout() -> std::time::Duration {
    let reasoner_ms = std::env::var("AURA_MODEL_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(REASONER_DEFAULT_TIMEOUT_MS);
    std::time::Duration::from_millis(reasoner_ms)
        + std::time::Duration::from_secs(STREAM_TIMEOUT_MARGIN_SECS)
}

/// Map a model identifier to its maximum context window in tokens.
///
/// Phase B / Commit 3: relocated to [`aura_engine::context_window_for_model`].
/// The gateway-side session bootstrap continues to call into the
/// engine's helper so the chat WS path and the automaton bridge see
/// identical token windows for the same model id.
pub(crate) use aura_engine::context_window_for_model;

#[cfg(test)]
mod tests;
