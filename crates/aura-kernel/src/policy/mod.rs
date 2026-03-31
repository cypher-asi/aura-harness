//! Policy engine for authorizing proposals and tool usage.
//!
//! ## Permission Levels
//!
//! Tools have different permission levels:
//! - `AlwaysAllow`: Safe read-only operations
//! - `AskOnce`: Requires approval once per session
//! - `AlwaysAsk`: Requires approval for each use
//! - `Deny`: Never allowed

use aura_core::{ActionKind, Proposal, ToolCall};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tracing::{debug, warn};

// ============================================================================
// Permission Levels
// ============================================================================

/// Permission level for tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionLevel {
    /// Always allowed without asking
    AlwaysAllow,
    /// Ask once per session, then remember
    AskOnce,
    /// Always ask before each use
    AlwaysAsk,
    /// Never allowed
    Deny,
}

/// Default permission level for a tool based on its name.
#[must_use]
pub fn default_tool_permission(tool: &str) -> PermissionLevel {
    match tool {
        "list_files" | "read_file" | "stat_file" | "search_code" | "run_command" => {
            PermissionLevel::AlwaysAllow
        }
        "write_file" | "edit_file" => PermissionLevel::AskOnce,
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
    /// Maximum proposals per request. Exposed via [`Policy::max_proposals`]; the kernel truncates proposals exceeding this limit.
    pub max_proposals: usize,
    /// Custom permission overrides for specific tools
    pub tool_permissions: HashMap<String, PermissionLevel>,
    /// When true, tools not in `allowed_tools` or `tool_permissions` get
    /// `AlwaysAllow` instead of `Deny`. The kernel is the sole gateway, so
    /// the default is open; use [`PolicyConfig::restrictive`] to lock down.
    pub allow_unlisted: bool,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        let mut allowed_action_kinds = HashSet::new();
        allowed_action_kinds.insert(ActionKind::Reason);
        allowed_action_kinds.insert(ActionKind::Memorize);
        allowed_action_kinds.insert(ActionKind::Decide);
        allowed_action_kinds.insert(ActionKind::Delegate);

        let mut allowed_tools = HashSet::new();
        allowed_tools.insert("list_files".to_string());
        allowed_tools.insert("read_file".to_string());
        allowed_tools.insert("stat_file".to_string());
        allowed_tools.insert("search_code".to_string());
        allowed_tools.insert("write_file".to_string());
        allowed_tools.insert("edit_file".to_string());
        allowed_tools.insert("run_command".to_string());

        Self {
            allowed_action_kinds,
            allowed_tools,
            max_proposals: 8,
            tool_permissions: HashMap::new(),
            allow_unlisted: true,
        }
    }
}

impl PolicyConfig {
    /// Create a permissive config that allows all tools.
    #[must_use]
    pub fn permissive() -> Self {
        Self::default()
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
}

// ============================================================================
// Policy Engine
// ============================================================================

/// Policy engine for authorizing proposals and tool usage.
///
/// Uses `std::sync::Mutex` for `session_approvals` intentionally: all
/// accesses are brief `insert`/`contains`/`remove`/`clear` with no
/// `.await` held across the lock, so a sync mutex avoids the overhead
/// of `tokio::sync::Mutex` and the `Send` bound it would impose on
/// callers.
#[derive(Debug)]
pub struct Policy {
    config: PolicyConfig,
    session_approvals: Mutex<HashSet<String>>,
}

/// Result of policy check.
#[derive(Debug)]
pub struct PolicyResult {
    /// Whether the proposal is allowed
    pub allowed: bool,
    /// Reason for rejection (if not allowed)
    pub reason: Option<String>,
}

impl Policy {
    /// Create a new policy with the given config.
    #[must_use]
    pub fn new(config: PolicyConfig) -> Self {
        Self {
            config,
            session_approvals: Mutex::new(HashSet::new()),
        }
    }

    /// Create a policy with default config.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(PolicyConfig::default())
    }

    /// Get the permission level for a tool.
    #[must_use]
    pub fn check_tool_permission(&self, tool: &str) -> PermissionLevel {
        if let Some(level) = self.config.tool_permissions.get(tool) {
            return *level;
        }

        if self.config.allowed_tools.contains(tool) {
            return default_tool_permission(tool);
        }

        if self.config.allow_unlisted {
            return PermissionLevel::AlwaysAllow;
        }

        PermissionLevel::Deny
    }

    /// Check if a tool is approved for this session.
    ///
    /// Recovers gracefully from mutex poisoning by accessing the inner data.
    #[must_use]
    pub fn is_session_approved(&self, tool: &str) -> bool {
        self.session_approvals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(tool)
    }

    /// Approve a tool for this session.
    ///
    /// Recovers gracefully from mutex poisoning by accessing the inner data.
    pub fn approve_for_session(&self, tool: &str) {
        self.session_approvals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(tool.to_string());
    }

    /// Revoke session approval for a tool.
    ///
    /// Recovers gracefully from mutex poisoning by accessing the inner data.
    pub fn revoke_session_approval(&self, tool: &str) {
        self.session_approvals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(tool);
    }

    /// Clear all session approvals.
    ///
    /// Recovers gracefully from mutex poisoning by accessing the inner data.
    pub fn clear_session_approvals(&self) {
        self.session_approvals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Check if a tool call requires approval.
    #[must_use]
    pub fn requires_approval(&self, tool: &str) -> bool {
        let permission = self.check_tool_permission(tool);
        match permission {
            PermissionLevel::AlwaysAllow => false,
            PermissionLevel::AskOnce => !self.is_session_approved(tool),
            PermissionLevel::AlwaysAsk | PermissionLevel::Deny => true,
        }
    }

    /// Check if a proposal is allowed.
    #[must_use]
    pub fn check(&self, proposal: &Proposal) -> PolicyResult {
        if !self
            .config
            .allowed_action_kinds
            .contains(&proposal.action_kind)
        {
            warn!(kind = ?proposal.action_kind, "Action kind not allowed");
            return PolicyResult {
                allowed: false,
                reason: Some(format!(
                    "Action kind {:?} not allowed",
                    proposal.action_kind
                )),
            };
        }

        if proposal.action_kind == ActionKind::Delegate {
            if let Ok(tool_call) = serde_json::from_slice::<ToolCall>(&proposal.payload) {
                let permission = self.check_tool_permission(&tool_call.tool);

                match permission {
                    PermissionLevel::Deny => {
                        warn!(tool = %tool_call.tool, "Tool denied by policy");
                        return PolicyResult {
                            allowed: false,
                            reason: Some(format!("Tool '{}' not allowed", tool_call.tool)),
                        };
                    }
                    PermissionLevel::AlwaysAsk => {
                        warn!(tool = %tool_call.tool, "Tool requires per-use approval");
                        return PolicyResult {
                            allowed: false,
                            reason: Some(format!(
                                "Tool '{}' requires approval for each use",
                                tool_call.tool
                            )),
                        };
                    }
                    PermissionLevel::AskOnce => {
                        if !self.is_session_approved(&tool_call.tool) {
                            warn!(tool = %tool_call.tool, "Tool requires session approval");
                            return PolicyResult {
                                allowed: false,
                                reason: Some(format!(
                                    "Tool '{}' requires approval",
                                    tool_call.tool
                                )),
                            };
                        }
                    }
                    PermissionLevel::AlwaysAllow => {
                        debug!(tool = %tool_call.tool, "Tool always allowed");
                    }
                }
            } else {
                warn!("Malformed delegate payload");
                return PolicyResult {
                    allowed: false,
                    reason: Some("Malformed delegate payload".to_string()),
                };
            }
        }

        debug!(kind = ?proposal.action_kind, "Proposal allowed");
        PolicyResult {
            allowed: true,
            reason: None,
        }
    }

    /// Check if a tool call is allowed (includes session approval check).
    #[must_use]
    pub fn check_tool(&self, tool: &str, _input: &serde_json::Value) -> PolicyResult {
        let permission = self.check_tool_permission(tool);

        match permission {
            PermissionLevel::Deny => PolicyResult {
                allowed: false,
                reason: Some(format!("Tool '{tool}' is not allowed")),
            },
            PermissionLevel::AlwaysAllow => PolicyResult {
                allowed: true,
                reason: None,
            },
            PermissionLevel::AskOnce => {
                if self.is_session_approved(tool) {
                    PolicyResult {
                        allowed: true,
                        reason: None,
                    }
                } else {
                    PolicyResult {
                        allowed: false,
                        reason: Some(format!("Tool '{tool}' requires approval")),
                    }
                }
            }
            PermissionLevel::AlwaysAsk => PolicyResult {
                allowed: false,
                reason: Some(format!("Tool '{tool}' requires approval for each use")),
            },
        }
    }

    /// Get maximum allowed proposals.
    #[must_use]
    pub const fn max_proposals(&self) -> usize {
        self.config.max_proposals
    }

    /// Add installed tool names to the policy's allowed set with `AlwaysAllow`.
    pub fn add_allowed_tools(&mut self, names: impl IntoIterator<Item = impl Into<String>>) {
        self.config.add_allowed_tools(names);
    }
}

#[cfg(test)]
mod tests;
