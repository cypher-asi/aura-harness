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
        // Safe read-only operations + command execution (autonomous operation)
        "fs_ls" | "fs_read" | "fs_stat" | "search_code" | "cmd_run" => PermissionLevel::AlwaysAllow,

        // Write operations need confirmation once per session
        "fs_write" | "fs_edit" => PermissionLevel::AskOnce,

        // Unknown tools are denied by default
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
    /// Maximum proposals per request
    pub max_proposals: usize,
    /// Custom permission overrides for specific tools
    pub tool_permissions: HashMap<String, PermissionLevel>,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        let mut allowed_action_kinds = HashSet::new();
        allowed_action_kinds.insert(ActionKind::Reason);
        allowed_action_kinds.insert(ActionKind::Memorize);
        allowed_action_kinds.insert(ActionKind::Decide);
        allowed_action_kinds.insert(ActionKind::Delegate);

        let mut allowed_tools = HashSet::new();
        // Read-only tools always allowed
        allowed_tools.insert("fs_ls".to_string());
        allowed_tools.insert("fs_read".to_string());
        allowed_tools.insert("fs_stat".to_string());
        allowed_tools.insert("search_code".to_string());
        // Write tools allowed but require approval
        allowed_tools.insert("fs_write".to_string());
        allowed_tools.insert("fs_edit".to_string());
        // Command execution
        allowed_tools.insert("cmd_run".to_string());

        Self {
            allowed_action_kinds,
            allowed_tools,
            max_proposals: 8,
            tool_permissions: HashMap::new(),
        }
    }
}

impl PolicyConfig {
    /// Create a permissive config that allows all tools.
    #[must_use]
    pub fn permissive() -> Self {
        // Default config now includes cmd_run
        Self::default()
    }

    /// Create a restrictive config with only read-only tools.
    #[must_use]
    pub fn restrictive() -> Self {
        let mut allowed_tools = HashSet::new();
        allowed_tools.insert("fs_ls".to_string());
        allowed_tools.insert("fs_read".to_string());
        allowed_tools.insert("fs_stat".to_string());
        allowed_tools.insert("search_code".to_string());

        Self {
            allowed_tools,
            ..Self::default()
        }
    }

    /// Set a custom permission level for a tool.
    #[must_use]
    pub fn with_tool_permission(mut self, tool: &str, level: PermissionLevel) -> Self {
        self.tool_permissions.insert(tool.to_string(), level);
        self
    }
}

// ============================================================================
// Policy Engine
// ============================================================================

/// Policy engine for authorizing proposals and tool usage.
#[derive(Debug)]
pub struct Policy {
    config: PolicyConfig,
    /// Session approvals for `AskOnce` tools
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
        // Check custom overrides first
        if let Some(level) = self.config.tool_permissions.get(tool) {
            return *level;
        }

        // Check if tool is in allowed list
        if !self.config.allowed_tools.contains(tool) {
            return PermissionLevel::Deny;
        }

        // Use default permission for the tool
        default_tool_permission(tool)
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
            // AlwaysAsk and Deny both require approval (Deny will be rejected regardless)
            PermissionLevel::AlwaysAsk | PermissionLevel::Deny => true,
        }
    }

    /// Check if a proposal is allowed.
    #[must_use]
    pub fn check(&self, proposal: &Proposal) -> PolicyResult {
        // Check action kind
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

        // For Delegate actions, check tool allowlist
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
                        // Allow but will require approval at execution time
                        debug!(tool = %tool_call.tool, "Tool will require approval");
                    }
                    PermissionLevel::AskOnce => {
                        if !self.is_session_approved(&tool_call.tool) {
                            debug!(tool = %tool_call.tool, "Tool requires session approval");
                        }
                    }
                    PermissionLevel::AlwaysAllow => {
                        debug!(tool = %tool_call.tool, "Tool always allowed");
                    }
                }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn test_default_permissions() {
        assert_eq!(
            default_tool_permission("fs_read"),
            PermissionLevel::AlwaysAllow
        );
        assert_eq!(
            default_tool_permission("fs_write"),
            PermissionLevel::AskOnce
        );
        assert_eq!(
            default_tool_permission("cmd_run"),
            PermissionLevel::AlwaysAllow
        );
        assert_eq!(
            default_tool_permission("unknown_tool"),
            PermissionLevel::Deny
        );
    }

    #[test]
    fn test_policy_allows_reason() {
        let policy = Policy::with_defaults();
        let proposal = Proposal::new(ActionKind::Reason, Bytes::new());

        let result = policy.check(&proposal);
        assert!(result.allowed);
    }

    #[test]
    fn test_policy_allows_fs_read() {
        let policy = Policy::with_defaults();
        let tool_call = ToolCall::fs_read("test.txt", None);
        let payload = serde_json::to_vec(&tool_call).unwrap();
        let proposal = Proposal::new(ActionKind::Delegate, Bytes::from(payload));

        let result = policy.check(&proposal);
        assert!(result.allowed);
    }

    #[test]
    fn test_policy_blocks_unknown_tool() {
        let policy = Policy::with_defaults();
        let tool_call = ToolCall::new("unknown.tool", serde_json::json!({}));
        let payload = serde_json::to_vec(&tool_call).unwrap();
        let proposal = Proposal::new(ActionKind::Delegate, Bytes::from(payload));

        let result = policy.check(&proposal);
        assert!(!result.allowed);
    }

    #[test]
    fn test_session_approvals() {
        let policy = Policy::with_defaults();

        // fs_write requires approval initially
        assert!(policy.requires_approval("fs_write"));

        // Approve for session
        policy.approve_for_session("fs_write");

        // Now it's approved
        assert!(!policy.requires_approval("fs_write"));
        assert!(policy.is_session_approved("fs_write"));

        // Clear approvals
        policy.clear_session_approvals();
        assert!(policy.requires_approval("fs_write"));
    }

    #[test]
    fn test_permission_override() {
        let config =
            PolicyConfig::default().with_tool_permission("fs_read", PermissionLevel::AskOnce);

        let policy = Policy::new(config);

        // Should use override, not default
        assert_eq!(
            policy.check_tool_permission("fs_read"),
            PermissionLevel::AskOnce
        );
    }

    #[test]
    fn test_restrictive_config() {
        let config = PolicyConfig::restrictive();
        let policy = Policy::new(config);

        // Read-only tools allowed
        assert_eq!(
            policy.check_tool_permission("fs_read"),
            PermissionLevel::AlwaysAllow
        );

        // Write tools denied
        assert_eq!(
            policy.check_tool_permission("fs_write"),
            PermissionLevel::Deny
        );
    }

    #[test]
    fn test_revoke_session_approval() {
        let policy = Policy::with_defaults();

        policy.approve_for_session("fs_write");
        assert!(policy.is_session_approved("fs_write"));

        policy.revoke_session_approval("fs_write");
        assert!(!policy.is_session_approved("fs_write"));
        assert!(policy.requires_approval("fs_write"));
    }

    #[test]
    fn test_clear_session_approvals_multiple() {
        let policy = Policy::with_defaults();

        policy.approve_for_session("fs_write");
        policy.approve_for_session("fs_edit");
        assert!(policy.is_session_approved("fs_write"));
        assert!(policy.is_session_approved("fs_edit"));

        policy.clear_session_approvals();
        assert!(!policy.is_session_approved("fs_write"));
        assert!(!policy.is_session_approved("fs_edit"));
    }

    #[test]
    fn test_revoke_nonexistent_approval_is_noop() {
        let policy = Policy::with_defaults();
        policy.revoke_session_approval("fs_write");
        assert!(!policy.is_session_approved("fs_write"));
    }

    #[test]
    fn test_always_allow_does_not_require_approval() {
        let policy = Policy::with_defaults();
        assert!(!policy.requires_approval("fs_read"));
        assert!(!policy.requires_approval("fs_ls"));
        assert!(!policy.requires_approval("cmd_run"));
    }

    #[test]
    fn test_denied_tool_requires_approval() {
        let policy = Policy::with_defaults();
        assert!(policy.requires_approval("some_unknown_tool"));
    }

    #[test]
    fn test_check_tool_always_allow() {
        let policy = Policy::with_defaults();
        let result = policy.check_tool("fs_read", &serde_json::json!({}));
        assert!(result.allowed);
        assert!(result.reason.is_none());
    }

    #[test]
    fn test_check_tool_denied() {
        let policy = Policy::with_defaults();
        let result = policy.check_tool("evil_tool", &serde_json::json!({}));
        assert!(!result.allowed);
        assert!(result.reason.unwrap().contains("not allowed"));
    }

    #[test]
    fn test_check_tool_ask_once_not_approved() {
        let policy = Policy::with_defaults();
        let result = policy.check_tool("fs_write", &serde_json::json!({}));
        assert!(!result.allowed);
        assert!(result.reason.unwrap().contains("requires approval"));
    }

    #[test]
    fn test_check_tool_ask_once_after_approval() {
        let policy = Policy::with_defaults();
        policy.approve_for_session("fs_write");
        let result = policy.check_tool("fs_write", &serde_json::json!({}));
        assert!(result.allowed);
    }

    #[test]
    fn test_always_ask_permission_override() {
        let config = PolicyConfig::default()
            .with_tool_permission("fs_read", PermissionLevel::AlwaysAsk);
        let policy = Policy::new(config);

        let result = policy.check_tool("fs_read", &serde_json::json!({}));
        assert!(!result.allowed);
        assert!(result
            .reason
            .unwrap()
            .contains("requires approval for each use"));
    }

    #[test]
    fn test_max_proposals() {
        let policy = Policy::with_defaults();
        assert_eq!(policy.max_proposals(), 8);
    }

    #[test]
    fn test_permissive_config_includes_cmd_run() {
        let config = PolicyConfig::permissive();
        assert!(config.allowed_tools.contains("cmd_run"));
    }

    #[test]
    fn test_concurrent_session_approvals() {
        use std::sync::Arc;
        let policy = Arc::new(Policy::with_defaults());

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let p = Arc::clone(&policy);
                std::thread::spawn(move || {
                    let tool = format!("tool_{i}");
                    p.approve_for_session(&tool);
                    assert!(p.is_session_approved(&tool));
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn test_check_proposal_disallowed_action_kind() {
        let mut allowed = HashSet::new();
        allowed.insert(ActionKind::Reason);
        let config = PolicyConfig {
            allowed_action_kinds: allowed,
            ..PolicyConfig::default()
        };
        let policy = Policy::new(config);

        let proposal = Proposal::new(ActionKind::Delegate, Bytes::new());
        let result = policy.check(&proposal);
        assert!(!result.allowed);
        assert!(result.reason.unwrap().contains("not allowed"));
    }
}
