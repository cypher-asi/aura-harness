//! Policy configuration: [`PermissionLevel`], [`PolicyConfig`], defaults.
//!
//! All behavior-free, data-shape pieces of the policy engine live here so
//! [`super::check`] can focus on the authorization pipeline itself.

use aura_core::{
    ActionKind, AgentPermissions, Capability, InstalledIntegrationDefinition,
    InstalledToolIntegrationRequirement,
};
use std::collections::{HashMap, HashSet};

// ============================================================================
// Permission Levels
// ============================================================================

// `PermissionLevel` moved to `aura_core::types::permission` so crates on
// the "outside" of the kernel (notably the `DomainApi` in `aura-tools`
// that fetches per-agent overrides from aura-network) can marshal these
// values without pulling in the kernel itself. Re-exported here so
// `aura_kernel::PermissionLevel` and `aura_kernel::policy::PermissionLevel`
// keep working for existing callers.
pub use aura_core::PermissionLevel;

/// Default permission level for a tool based on its name.
///
/// Read-only and narrow filesystem tools default to `AlwaysAllow`.
/// `run_command` defaults to [`PermissionLevel::RequireApproval`] (Wave 5 /
/// T3.3, renamed in Phase 6 / security audit): spawning arbitrary shell
/// commands is the biggest blast-radius tool the kernel exposes, so
/// every invocation must be explicitly pre-approved via
/// [`crate::Kernel::grant_approval`]. Hosts that trust the running
/// agent (e.g. headless CI) can still flip this to `AlwaysAllow` via
/// [`PolicyConfig::tool_permissions`].
#[must_use]
pub fn default_tool_permission(tool: &str) -> PermissionLevel {
    match tool {
        // Read-only discovery / content tools, plus the core edit and
        // delete verbs. `find_files` was historically missing from this
        // arm (followup to the Phase 5 audit) which made it permanently
        // `Deny` even for hosts that spliced it into `allowed_tools`.
        "list_files" | "find_files" | "read_file" | "stat_file" | "search_code" | "write_file"
        | "edit_file" | "delete_file" => PermissionLevel::AlwaysAllow,
        "run_command" => PermissionLevel::RequireApproval,
        _ => PermissionLevel::Deny,
    }
}

// ============================================================================
// Policy Configuration
// ============================================================================

/// Policy configuration.
#[derive(Debug, Clone)]
pub struct PolicyConfig {
    /// Allowed action kinds
    pub allowed_action_kinds: HashSet<ActionKind>,
    /// Allowed tools
    pub allowed_tools: HashSet<String>,
    /// Maximum proposals per request. Exposed via [`super::Policy::max_proposals`]; the kernel truncates proposals exceeding this limit.
    pub max_proposals: usize,
    /// Custom permission overrides for specific tools
    pub tool_permissions: HashMap<String, PermissionLevel>,
    /// Installed integrations currently authorized for this runtime.
    pub installed_integrations: Vec<InstalledIntegrationDefinition>,
    /// Declared integration requirements for tools.
    pub tool_integration_requirements: HashMap<String, InstalledToolIntegrationRequirement>,
    /// When true, tools not in `allowed_tools` or `tool_permissions` get
    /// `AlwaysAllow` instead of `Deny`. **Defaults to `false`** (Phase 5
    /// hardening — closes finding C5): unlisted tools fall through to
    /// `Deny` so adding a tool to the runtime registry no longer
    /// auto-grants it at policy-check time. Hosts that deliberately
    /// want a wide-open policy (e.g. a controlled test harness) can
    /// flip this back to `true` explicitly.
    pub allow_unlisted: bool,
    /// Scope + capability bundle for the agent this policy governs.
    /// Always consulted on `Delegate` proposals — the check is
    /// unconditional and cannot be disabled. [`AgentPermissions::empty`]
    /// denies every capability-gated tool; [`AgentPermissions::ceo_preset`]
    /// grants everything.
    pub agent_permissions: AgentPermissions,
    /// Mapping from tool name to the [`Capability`] required to use it.
    /// Tools not listed here carry no capability requirement.
    pub tool_capability_requirements: HashMap<String, Capability>,
}

impl Default for PolicyConfig {
    /// Fail-closed defaults (Phase 5 hardening — closes finding C5):
    ///
    /// * `allow_unlisted` is `false`, so anything not in `allowed_tools`
    ///   and not explicitly named in `tool_permissions` is denied.
    /// * `run_command` is **not** pre-populated in `allowed_tools`.
    ///   `default_tool_permission("run_command")` still returns
    ///   `RequireApproval`, so a host that opts in by inserting `run_command`
    ///   continues to require per-invocation approval via
    ///   [`crate::Kernel::grant_approval`].
    fn default() -> Self {
        let mut allowed_action_kinds = HashSet::new();
        allowed_action_kinds.insert(ActionKind::Reason);
        allowed_action_kinds.insert(ActionKind::Memorize);
        allowed_action_kinds.insert(ActionKind::Decide);
        allowed_action_kinds.insert(ActionKind::Delegate);

        let mut allowed_tools = HashSet::new();
        allowed_tools.insert("list_files".to_string());
        allowed_tools.insert("find_files".to_string());
        allowed_tools.insert("read_file".to_string());
        allowed_tools.insert("stat_file".to_string());
        allowed_tools.insert("search_code".to_string());
        allowed_tools.insert("write_file".to_string());
        allowed_tools.insert("edit_file".to_string());
        // `delete_file` already had `AlwaysAllow` in
        // `default_tool_permission` but was missing from this set,
        // making it effectively `Deny` under `allow_unlisted = false`.
        // Reconcile the two so the Phase-5 fail-closed default stops
        // contradicting itself.
        allowed_tools.insert("delete_file".to_string());

        Self {
            allowed_action_kinds,
            allowed_tools,
            max_proposals: 8,
            tool_permissions: HashMap::new(),
            installed_integrations: Vec::new(),
            tool_integration_requirements: HashMap::new(),
            allow_unlisted: false,
            agent_permissions: AgentPermissions::empty(),
            tool_capability_requirements: HashMap::new(),
        }
    }
}

impl PolicyConfig {
    /// Create a permissive config that explicitly opens the policy:
    /// `allow_unlisted = true` and `run_command` is added to
    /// `allowed_tools`. Kept distinct from [`Self::default`] after the
    /// Phase 5 fail-closed flip so callers who actually want the wide
    /// gate still have a one-liner.
    #[must_use]
    pub fn permissive() -> Self {
        let mut cfg = Self::default();
        cfg.allowed_tools.insert("run_command".to_string());
        cfg.allow_unlisted = true;
        cfg
    }

    /// Create a restrictive config with only read-only tools.
    /// Unlisted tools are denied.
    #[must_use]
    pub fn restrictive() -> Self {
        let mut allowed_tools = HashSet::new();
        allowed_tools.insert("list_files".to_string());
        allowed_tools.insert("read_file".to_string());
        allowed_tools.insert("stat_file".to_string());
        allowed_tools.insert("search_code".to_string());

        Self {
            allowed_tools,
            allow_unlisted: false,
            ..Self::default()
        }
    }

    /// Set a custom permission level for a tool.
    #[must_use]
    pub fn with_tool_permission(mut self, tool: &str, level: PermissionLevel) -> Self {
        self.tool_permissions.insert(tool.to_string(), level);
        self
    }

    /// Add a single tool to the allowed set with `AlwaysAllow` permission.
    pub fn add_allowed_tool(&mut self, name: impl Into<String>) {
        let name = name.into();
        self.allowed_tools.insert(name.clone());
        self.tool_permissions
            .insert(name, PermissionLevel::AlwaysAllow);
    }

    /// Add multiple tools to the allowed set with `AlwaysAllow` permission.
    pub fn add_allowed_tools(&mut self, names: impl IntoIterator<Item = impl Into<String>>) {
        for name in names {
            self.add_allowed_tool(name);
        }
    }

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

    /// Declare the [`Capability`] required to invoke `tool`.
    #[must_use]
    pub fn with_tool_capability(mut self, tool: impl Into<String>, cap: Capability) -> Self {
        self.tool_capability_requirements.insert(tool.into(), cap);
        self
    }
}
