//! Session state management.
//!
//! This file owns the [`Session`] struct plus everything that maintains
//! its per-connection state: `new`, `apply_init`, the wire→core permission
//! translator, the intent-classifier builder, `AgentLoopConfig` derivation,
//! and the message-buffer truncation that caps large tool blobs between
//! turns. Split out of `session/mod.rs` in Wave 6 / T3 so `mod.rs` can stay
//! tiny (declarations + `WsContext` + re-exports).

use crate::protocol::{self, SessionInit};
use aura_agent::{prompts::default_system_prompt, AgentLoopConfig};
use aura_core::{
    AgentId, AgentPermissions, AgentScope, Capability, InstalledIntegrationDefinition,
    InstalledToolDefinition,
};
use aura_protocol::{
    AgentPermissionsWire, CapabilityWire, IntentClassifierSpec, SessionProviderConfig,
};
use aura_reasoner::{Message, ModelProvider, ToolDefinition};
use aura_tools::IntentClassifier;
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
/// purpose. (Wave 3 — T2.2.)
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
    /// Optional provider override resolved from `session_init`.
    pub(crate) provider_config: Option<SessionProviderConfig>,
    /// Optional concrete provider override built from `provider_config`.
    pub(crate) provider_override: Option<Arc<dyn ModelProvider + Send + Sync>>,
    /// Max tokens per response.
    pub(crate) max_tokens: u32,
    /// Sampling temperature.
    pub(crate) temperature: Option<f32>,
    /// Maximum agentic steps per turn.
    pub(crate) max_turns: u32,
    /// Installed tools registered for this session.
    pub(crate) installed_tools: Vec<InstalledToolDefinition>,
    /// Installed integrations authorized for this session.
    pub(crate) installed_integrations: Vec<InstalledIntegrationDefinition>,
    /// Conversation history (accumulated across turns).
    pub(crate) messages: Vec<Message>,
    /// Cumulative input tokens across all turns.
    pub(crate) cumulative_input_tokens: u64,
    /// Cumulative output tokens across all turns.
    pub(crate) cumulative_output_tokens: u64,
    /// Cumulative cache creation input tokens across all turns.
    pub(crate) cumulative_cache_creation_input_tokens: u64,
    /// Cumulative cache read input tokens across all turns.
    pub(crate) cumulative_cache_read_input_tokens: u64,
    /// Workspace directory for this session (sandboxed fallback).
    pub(crate) workspace: PathBuf,
    /// Base directory that workspace must reside under.
    pub(crate) workspace_base: PathBuf,
    /// Real project directory on the host filesystem.
    /// When set, tool execution uses this path directly.
    pub(crate) project_path: Option<PathBuf>,
    /// Optional base directory that project_path must reside under (remote VM mode).
    pub(super) project_base: Option<PathBuf>,
    /// Whether `session_init` has been received.
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
    /// Optional keyword-driven intent classifier that narrows the visible
    /// tool set per turn. Populated from
    /// [`aura_protocol::SessionInit::intent_classifier`] so a
    /// harness-hosted super-agent can reproduce the aura-os tier-1/tier-2
    /// filtering behavior without the harness binary knowing the manifest.
    pub(crate) intent_classifier: Option<Arc<IntentClassifier>>,
    /// `(tool_name, domain)` pairs paired with [`intent_classifier`]. Empty
    /// when the classifier is not configured.
    ///
    /// [`intent_classifier`]: Self::intent_classifier
    pub(crate) intent_classifier_manifest: Vec<(String, String)>,
    /// Agent permissions for this session, derived directly from the
    /// required `SessionInit.agent_permissions` field. Always applied to
    /// the kernel [`aura_kernel::PolicyConfig`] on kernel construction;
    /// enforcement is unconditional.
    pub(crate) agent_permissions: AgentPermissions,
}

impl Session {
    /// Create a new uninitialized session with defaults.
    pub(super) fn new(default_workspace: PathBuf) -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            agent_id: AgentId::generate(),
            system_prompt: String::new(),
            model: aura_agent::DEFAULT_MODEL.to_string(),
            provider_name: String::new(),
            provider_config: None,
            provider_override: None,
            max_tokens: 16384,
            temperature: None,
            max_turns: 25,
            installed_tools: Vec::new(),
            installed_integrations: Vec::new(),
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
            skill_agent_id: None,
            intent_classifier: None,
            intent_classifier_manifest: Vec::new(),
            agent_permissions: AgentPermissions::empty(),
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
        if let Some(integrations) = init.installed_integrations {
            self.installed_integrations = integrations
                .into_iter()
                .map(protocol::installed_integration_to_core)
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
                    return Err(format!("project_path must be under {}", base.display()));
                }
            }
            self.project_path = Some(candidate);
        }
        if let Some(token) = init.token {
            self.auth_token = Some(token);
        }
        if let Some(agent_id) = init.agent_id {
            self.skill_agent_id = Some(agent_id.clone());
            self.agent_id = AgentId::from_hex(&agent_id).unwrap_or_else(|_| {
                let hash = blake3::hash(agent_id.as_bytes());
                AgentId::new(*hash.as_bytes())
            });
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
        if let Some(provider_config) = init.provider_config {
            self.provider_config = Some(provider_config);
        }
        if let Some(spec) = init.intent_classifier {
            let (classifier, manifest) = build_intent_classifier(spec);
            self.intent_classifier = Some(Arc::new(classifier));
            self.intent_classifier_manifest = manifest;
        }

        // Agent permissions are a required field on `SessionInit` and are
        // applied verbatim to the session — there is no role-based
        // fallback, no named preset, and no legacy off-switch. Mid-session
        // changes are rejected at the `/tx` layer; `apply_init` is only
        // called once per session (see `initialized` guard in
        // `handle_session_init`).
        self.agent_permissions = agent_permissions_from_wire(init.agent_permissions);
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

    /// Return a deterministic `AgentId` for memory keying.
    ///
    /// When the session carries an `aura_agent_id` (the aura-os UUID),
    /// derive the `AgentId` from it so memory queries from the UI use the
    /// same key. Falls back to the random session `agent_id`.
    pub(super) fn memory_agent_id(&self) -> AgentId {
        if let Some(ref uuid_str) = self.aura_agent_id {
            if let Ok(uuid) = uuid::Uuid::parse_str(uuid_str) {
                return AgentId::from_uuid(uuid);
            }
        }
        self.agent_id
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
            stream_timeout: std::time::Duration::from_secs(180),
            auth_token: self.auth_token.clone(),
            upstream_provider_family: self
                .provider_config
                .as_ref()
                .and_then(|config| config.upstream_provider_family.clone()),
            aura_project_id: self.project_id.clone(),
            aura_agent_id: self.aura_agent_id.clone(),
            aura_session_id: self.aura_session_id.clone(),
            aura_org_id: self.aura_org_id.clone(),
            intent_classifier: self.intent_classifier.clone(),
            intent_classifier_manifest: self.intent_classifier_manifest.clone(),
            ..AgentLoopConfig::default()
        }
    }
}

/// Hard upper bound on bytes-per-tool-blob kept in `Session.messages`
/// between turns. Large tool results (e.g. a verbose `list_agents`
/// dump) used to ride along with every subsequent turn because the
/// session's message log is append-only, which could push the next
/// cold-start prompt past the model's context window well before the
/// existing compaction tier ever fires (`select_tier` keys off the
/// *total* token estimate; a single 60KB blob can live happily under
/// that floor and still blow up the wire payload).
///
/// `truncate_messages_for_storage` walks the message log after every
/// completed turn and replaces any `ToolUse` input / `ToolResult`
/// content that exceeds this cap with a "... [truncated N bytes]"
/// marker. This runs in addition to — not instead of — the
/// utilization-based compaction in `aura_agent::compaction`; the two
/// are complementary (this bounds per-blob size, compaction bounds
/// total size).
pub(crate) const SESSION_TOOL_BLOB_MAX_BYTES: usize = 8 * 1024;

/// Cap each `ToolUse` input / `ToolResult` content in `messages` at
/// [`SESSION_TOOL_BLOB_MAX_BYTES`]. Mutates in place and is cheap when
/// nothing exceeds the cap (no allocation). Run this after a turn
/// completes so the next turn doesn't re-ship the full blob.
pub(crate) fn truncate_messages_for_storage(messages: &mut [Message]) {
    use aura_reasoner::{ContentBlock, ToolResultContent};

    fn truncate_str(s: &str, max: usize) -> Option<String> {
        if s.len() <= max {
            return None;
        }
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        Some(format!("{}... [truncated {} bytes]", &s[..end], s.len()))
    }

    for msg in messages.iter_mut() {
        for block in msg.content.iter_mut() {
            match block {
                ContentBlock::ToolUse { input, .. } => {
                    if let Ok(serialized) = serde_json::to_string(input) {
                        if let Some(truncated) =
                            truncate_str(&serialized, SESSION_TOOL_BLOB_MAX_BYTES)
                        {
                            *input = serde_json::Value::String(truncated);
                        }
                    }
                }
                ContentBlock::ToolResult { content, .. } => match content {
                    ToolResultContent::Text(t) => {
                        if let Some(truncated) = truncate_str(t, SESSION_TOOL_BLOB_MAX_BYTES) {
                            *t = truncated;
                        }
                    }
                    ToolResultContent::Json(v) => {
                        if let Ok(serialized) = serde_json::to_string(v) {
                            if let Some(truncated) =
                                truncate_str(&serialized, SESSION_TOOL_BLOB_MAX_BYTES)
                            {
                                *content = ToolResultContent::Text(truncated);
                            }
                        }
                    }
                },
                _ => {}
            }
        }
    }
}

/// Translate an [`IntentClassifierSpec`] from the wire protocol into the
/// in-process [`IntentClassifier`] plus a `(tool_name, domain)` manifest
/// the agent loop can consume.
///
/// Kept as a free function (rather than an `impl From`) so both sides of
/// the conversion stay obvious at call sites — the spec flattens a
/// `HashMap<String, String>` while the loop expects a stable `Vec` so
/// filters are deterministic.
fn build_intent_classifier(
    spec: IntentClassifierSpec,
) -> (IntentClassifier, Vec<(String, String)>) {
    let IntentClassifierSpec {
        tier1_domains,
        classifier_rules,
        tool_domains,
    } = spec;
    let rules: Vec<(String, Vec<String>)> = classifier_rules
        .into_iter()
        .map(|r| (r.domain, r.keywords))
        .collect();
    let mut manifest: Vec<(String, String)> = tool_domains.into_iter().collect();
    // Stable ordering keeps `build_request` deterministic even though
    // the classifier itself doesn't care about order.
    manifest.sort_by(|a, b| a.0.cmp(&b.0));
    (IntentClassifier::from_rules(tier1_domains, rules), manifest)
}

/// Phase 5: translate the wire `AgentPermissionsWire` into the harness-core
/// `AgentPermissions` used by tools + the kernel policy. Kept here (rather
/// than in `aura-protocol`) so the protocol crate stays decoupled from
/// harness internals — see the module doc on `aura_protocol::SessionInit`.
pub(crate) fn agent_permissions_from_wire(wire: AgentPermissionsWire) -> AgentPermissions {
    let capabilities = wire
        .capabilities
        .into_iter()
        .filter_map(|c| match c {
            CapabilityWire::SpawnAgent => Some(Capability::SpawnAgent),
            CapabilityWire::ControlAgent => Some(Capability::ControlAgent),
            CapabilityWire::ReadAgent => Some(Capability::ReadAgent),
            CapabilityWire::ManageOrgMembers => Some(Capability::ManageOrgMembers),
            CapabilityWire::ManageBilling => Some(Capability::ManageBilling),
            CapabilityWire::InvokeProcess => Some(Capability::InvokeProcess),
            CapabilityWire::PostToFeed => Some(Capability::PostToFeed),
            CapabilityWire::GenerateMedia => Some(Capability::GenerateMedia),
            CapabilityWire::ReadProject { id } => Some(Capability::ReadProject { id }),
            CapabilityWire::WriteProject { id } => Some(Capability::WriteProject { id }),
            CapabilityWire::ReadAllProjects => Some(Capability::ReadAllProjects),
            CapabilityWire::WriteAllProjects => Some(Capability::WriteAllProjects),
            // Forward-compat: a newer server can send capability variants
            // this harness build doesn't know yet. Per the protocol doc,
            // drop them silently rather than rejecting the session — the
            // tools that depend on them simply won't be enforceable here.
            CapabilityWire::Unknown => None,
        })
        .collect();
    AgentPermissions {
        scope: AgentScope {
            orgs: wire.scope.orgs,
            projects: wire.scope.projects,
            agent_ids: wire.scope.agent_ids,
        },
        capabilities,
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
