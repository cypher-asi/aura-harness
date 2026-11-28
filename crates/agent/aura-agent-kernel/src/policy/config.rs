//! Policy configuration: [`PolicyConfig`] and related policy shape types.
//!
//! All behavior-free, data-shape pieces of the policy engine live here so
//! [`super::check`] can focus on the authorization pipeline itself.

use super::check::ToolModeClass;
use aura_core_modes::{AgentMode, ApprovalLevel};
use aura_core_types::{
    ActionKind, AgentPermissions, AgentToolPermissions, Capability, InstalledIntegrationDefinition,
    InstalledToolIntegrationRequirement, UserToolDefaults,
};
use std::collections::{BTreeMap, HashMap, HashSet};

// ============================================================================
// Policy Configuration
// ============================================================================

/// Policy configuration.
#[derive(Debug, Clone)]
pub struct PolicyConfig {
    /// Allowed action kinds
    pub allowed_action_kinds: HashSet<ActionKind>,
    /// Maximum proposals per request. Exposed via [`super::Policy::max_proposals`]; the kernel truncates proposals exceeding this limit.
    pub max_proposals: usize,
    /// Installed integrations currently authorized for this runtime.
    pub installed_integrations: Vec<InstalledIntegrationDefinition>,
    /// Declared integration requirements for tools.
    pub tool_integration_requirements: HashMap<String, InstalledToolIntegrationRequirement>,
    /// Scope + capability bundle for the agent this policy governs.
    /// Always consulted on `Delegate` proposals â€” the check is
    /// unconditional and cannot be disabled. [`AgentPermissions::full_access`]
    /// grants everything; callers that need a locked-down agent must pass an
    /// explicit narrower bundle.
    pub agent_permissions: AgentPermissions,
    /// Mapping from tool name to the [`Capability`] required to use it.
    /// Tools not listed here carry no capability requirement.
    pub tool_capability_requirements: HashMap<String, Capability>,
    /// User-level tool permission defaults — the originating user's
    /// "default permissions" / "auto-review" / "full access" mode. Every
    /// `(agent, tool)` resolves through this plus the optional
    /// [`Self::agent_override`] via
    /// [`aura_core_types::resolve_effective_permission`]. Defaults to
    /// [`UserToolDefaults::full_access`] to preserve "default ON" for
    /// callers that do not load a persisted user profile.
    ///
    pub user_default: UserToolDefaults,
    /// Optional per-agent override map stamped on this agent's
    /// [`aura_core_types::Identity`]. `None` (or an empty map) means
    /// "inherit the user default verbatim". Populated entries override
    /// only that specific tool; anything unlisted still flows through
    /// [`Self::user_default`].
    pub agent_override: Option<AgentToolPermissions>,
    /// Session mode. Checked before every other gate on `Delegate`
    /// proposals and can only deny. Defaults to [`AgentMode::Agent`],
    /// which permits every action class.
    pub mode: AgentMode,
    /// Which allowed calls ask the user first. The per-tool states in
    /// [`Self::user_default`] carry the asking; this level adds what they
    /// cannot: under [`ApprovalLevel::Yolo`] commands run confined to
    /// writes inside the workspace.
    pub approval: ApprovalLevel,
    /// Mode classes of tools the built-in table
    /// ([`super::tool_mode_class`]) doesn't know, such as integration
    /// tools added at runtime. The built-in table wins; a tool in neither
    /// is a network tool.
    pub tool_classes: BTreeMap<String, ToolModeClass>,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        let mut allowed_action_kinds = HashSet::new();
        allowed_action_kinds.insert(ActionKind::Reason);
        allowed_action_kinds.insert(ActionKind::Memorize);
        allowed_action_kinds.insert(ActionKind::Decide);
        allowed_action_kinds.insert(ActionKind::Delegate);

        let mut tool_capability_requirements = HashMap::new();
        tool_capability_requirements.insert("spawn_agent".to_string(), Capability::SpawnAgent);
        tool_capability_requirements.insert("task".to_string(), Capability::SpawnAgent);
        tool_capability_requirements.insert("send_to_agent".to_string(), Capability::ControlAgent);
        tool_capability_requirements
            .insert("agent_lifecycle".to_string(), Capability::ControlAgent);
        tool_capability_requirements.insert("delegate_task".to_string(), Capability::ControlAgent);
        tool_capability_requirements.insert("get_agent_state".to_string(), Capability::ReadAgent);
        tool_capability_requirements.insert("list_agents".to_string(), Capability::ListAgents);
        tool_capability_requirements.insert("run_command".to_string(), Capability::InvokeProcess);
        tool_capability_requirements.insert("post_to_feed".to_string(), Capability::PostToFeed);
        tool_capability_requirements.insert("check_budget".to_string(), Capability::ManageBilling);
        tool_capability_requirements.insert("record_usage".to_string(), Capability::ManageBilling);
        tool_capability_requirements.insert("computer".to_string(), Capability::ComputerUse);

        Self {
            allowed_action_kinds,
            max_proposals: 8,
            installed_integrations: Vec::new(),
            tool_integration_requirements: HashMap::new(),
            agent_permissions: AgentPermissions::full_access(),
            tool_capability_requirements,
            user_default: UserToolDefaults::full_access(),
            agent_override: None,
            mode: AgentMode::Agent,
            approval: ApprovalLevel::default(),
            tool_classes: BTreeMap::new(),
        }
    }
}

impl PolicyConfig {
    /// Replace the installed integrations set for this runtime.
    pub fn set_installed_integrations(
        &mut self,
        integrations: impl IntoIterator<Item = InstalledIntegrationDefinition>,
    ) {
        self.installed_integrations = integrations.into_iter().collect();
    }

    /// Replace the tool-to-integration requirement map for this runtime.
    pub fn set_tool_integration_requirements(
        &mut self,
        requirements: impl IntoIterator<Item = (String, InstalledToolIntegrationRequirement)>,
    ) {
        self.tool_integration_requirements = requirements.into_iter().collect();
    }

    /// Attach an [`AgentPermissions`] bundle to this policy.
    #[must_use]
    pub fn with_agent_permissions(mut self, permissions: AgentPermissions) -> Self {
        self.agent_permissions = permissions;
        self
    }

    /// Add `classes` to [`Self::tool_classes`].
    #[must_use]
    pub fn with_tool_classes(
        mut self,
        classes: impl IntoIterator<Item = (String, ToolModeClass)>,
    ) -> Self {
        self.tool_classes.extend(classes);
        self
    }

    /// The mode class of `tool`: the built-in table's, else
    /// [`Self::tool_classes`]'s; `None` when neither knows it.
    #[must_use]
    pub fn tool_class(&self, tool: &str) -> Option<ToolModeClass> {
        super::tool_mode_class(tool).or_else(|| self.tool_classes.get(tool).copied())
    }

    /// Declare the [`Capability`] required to invoke `tool`.
    #[must_use]
    pub fn with_tool_capability(mut self, tool: impl Into<String>, cap: Capability) -> Self {
        self.tool_capability_requirements.insert(tool.into(), cap);
        self
    }

    /// Attach the originating user's tool-permission defaults. This is
    /// the first half of the tri-state (`on`/`off`/`ask`) resolution
    /// consulted by [`aura_core_types::resolve_effective_permission`].
    #[must_use]
    pub fn with_user_default(mut self, user_default: UserToolDefaults) -> Self {
        self.user_default = user_default;
        self
    }

    /// Attach the per-agent override map. `None` / empty map means
    /// "inherit the user default verbatim". Populated entries override
    /// only those specific tools; anything unlisted still flows through
    /// [`Self::user_default`].
    #[must_use]
    pub fn with_agent_override(mut self, agent_override: Option<AgentToolPermissions>) -> Self {
        self.agent_override = agent_override;
        self
    }

    /// Set the session [`AgentMode`].
    #[must_use]
    pub fn with_mode(mut self, mode: AgentMode) -> Self {
        self.mode = mode;
        self
    }

    /// BLAKE3 hex digest over a canonical JSON rendering of every field
    /// that can change a tool verdict. Map and set fields are sorted so
    /// equal configurations always hash equally.
    #[must_use]
    pub fn snapshot_hash(&self) -> String {
        let mut action_kinds: Vec<String> = self
            .allowed_action_kinds
            .iter()
            .map(|kind| format!("{kind:?}"))
            .collect();
        action_kinds.sort();
        let capability_requirements: BTreeMap<&String, String> = self
            .tool_capability_requirements
            .iter()
            .map(|(tool, cap)| (tool, format!("{cap:?}")))
            .collect();
        let integration_requirements: BTreeMap<&String, serde_json::Value> = self
            .tool_integration_requirements
            .iter()
            .map(|(tool, req)| (tool, serde_json::to_value(req).unwrap_or_default()))
            .collect();
        let mut integrations: Vec<serde_json::Value> = self
            .installed_integrations
            .iter()
            .map(|i| serde_json::to_value(i).unwrap_or_default())
            .collect();
        integrations.sort_by_key(ToString::to_string);
        let mut snapshot = serde_json::json!({
            "allowed_action_kinds": action_kinds,
            "max_proposals": self.max_proposals,
            "installed_integrations": integrations,
            "tool_integration_requirements": integration_requirements,
            "agent_permissions": self.agent_permissions,
            "tool_capability_requirements": capability_requirements,
            "user_default": self.user_default,
            "agent_override": self.agent_override,
            "mode": format!("{:?}", self.mode),
        });
        // Left out at the default so configurations from before approval
        // levels keep their hash.
        if self.approval != ApprovalLevel::default() {
            snapshot["approval"] = serde_json::json!(self.approval.name());
        }
        // Likewise left out when empty.
        if !self.tool_classes.is_empty() {
            let classes: BTreeMap<&String, String> = self
                .tool_classes
                .iter()
                .map(|(tool, class)| (tool, format!("{class:?}")))
                .collect();
            snapshot["tool_classes"] = serde_json::json!(classes);
        }
        blake3::hash(snapshot.to_string().as_bytes())
            .to_hex()
            .to_string()
    }
}
