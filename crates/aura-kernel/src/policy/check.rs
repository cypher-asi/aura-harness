//! Policy engine proper — the [`Policy`] type and its authorization
//! pipeline.
//!
//! `check.rs` holds everything that actively *evaluates* a [`Proposal`] or
//! [`ToolCall`] against the declarative [`super::config::PolicyConfig`].
//! Session-scoped approval state (`AskOnce` memo), tool permission
//! resolution, runtime-capability checks, and agent-permission scope
//! checks all live here.

use super::config::{default_tool_permission, PermissionLevel, PolicyConfig};
use aura_core::{ActionKind, Proposal, RuntimeCapabilityInstall, ToolCall};
use std::collections::HashSet;
use std::sync::Mutex;
use tracing::{debug, warn};

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
        self.check_with_runtime_capabilities(proposal, None)
    }

    /// Check if a proposal is allowed against an optional persisted runtime
    /// capability snapshot.
    #[must_use]
    pub fn check_with_runtime_capabilities(
        &self,
        proposal: &Proposal,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
    ) -> PolicyResult {
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
                let result = self.check_tool_with_runtime_capabilities(
                    &tool_call.tool,
                    &tool_call.args,
                    runtime_capabilities,
                );
                if !result.allowed {
                    return result;
                }

                if let Some(result) = self.check_agent_permissions(&tool_call) {
                    if !result.allowed {
                        return result;
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
        self.check_tool_with_runtime_capabilities(tool, _input, None)
    }

    /// Check if a tool call is allowed against an optional persisted runtime
    /// capability snapshot.
    #[must_use]
    pub fn check_tool_with_runtime_capabilities(
        &self,
        tool: &str,
        _input: &serde_json::Value,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
    ) -> PolicyResult {
        let integration_gate = self.integration_requirement_satisfied(tool, runtime_capabilities);
        let permission = self.check_tool_permission(tool);

        match permission {
            PermissionLevel::Deny => PolicyResult {
                allowed: false,
                reason: Some(format!("Tool '{tool}' is not allowed")),
            },
            PermissionLevel::AlwaysAllow => PolicyResult {
                allowed: integration_gate.is_none(),
                reason: integration_gate,
            },
            PermissionLevel::AskOnce => {
                if self.is_session_approved(tool) {
                    PolicyResult {
                        allowed: integration_gate.is_none(),
                        reason: integration_gate,
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

    /// Evaluate a [`ToolCall`] against the caller's [`AgentPermissions`].
    /// Returns `None` when the call passes, or `Some(rejection)` when the
    /// call is denied because the caller lacks the required capability or
    /// the args target an out-of-scope org / project / agent.
    ///
    /// Always on — there is no feature flag or opt-out.
    fn check_agent_permissions(&self, tool_call: &ToolCall) -> Option<PolicyResult> {
        let permissions = &self.config.agent_permissions;

        if let Some(required) = self
            .config
            .tool_capability_requirements
            .get(&tool_call.tool)
        {
            // Route through `Capability::satisfies` so project wildcards
            // (`ReadAllProjects` / `WriteAllProjects`) on the bundle cover
            // an exact-id `ReadProject { id }` / `WriteProject { id }`
            // tool requirement. Keeps harness kernel enforcement aligned
            // with `aura-os-agent-runtime::policy::holds_capability`.
            let held = permissions
                .capabilities
                .iter()
                .any(|held| held.satisfies(required));
            if !held {
                return Some(PolicyResult {
                    allowed: false,
                    reason: Some(format!("permissions: requires capability {required:?}")),
                });
            }
        }

        if let Some(reason) = scope_violation(&permissions.scope, &tool_call.args) {
            return Some(PolicyResult {
                allowed: false,
                reason: Some(reason),
            });
        }

        None
    }

    fn integration_requirement_satisfied(
        &self,
        tool: &str,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
    ) -> Option<String> {
        let required_integration = if let Some(runtime_capabilities) = runtime_capabilities {
            match runtime_capabilities.tool_capability(tool) {
                Some(tool_capability) => tool_capability.required_integration.as_ref(),
                None if self.config.tool_integration_requirements.contains_key(tool) => {
                    return Some(format!(
                        "Tool '{tool}' is not installed in the kernel runtime capability ledger"
                    ));
                }
                None => self.config.tool_integration_requirements.get(tool),
            }
        } else {
            self.config.tool_integration_requirements.get(tool)
        };
        let required_integration = required_integration?;

        let installed = runtime_capabilities.map_or_else(
            || {
                self.config
                    .installed_integrations
                    .iter()
                    .any(|integration| {
                        required_integration
                            .integration_id
                            .as_deref()
                            .map_or(true, |expected| integration.integration_id == expected)
                            && required_integration
                                .provider
                                .as_deref()
                                .map_or(true, |expected| integration.provider == expected)
                            && required_integration
                                .kind
                                .as_deref()
                                .map_or(true, |expected| integration.kind == expected)
                    })
            },
            |runtime_capabilities| {
                runtime_capabilities.integration_requirement_satisfied(required_integration)
            },
        );

        if installed {
            None
        } else {
            Some(format!(
                "Tool '{tool}' requires an installed integration{}{}{}",
                required_integration
                    .provider
                    .as_deref()
                    .map(|provider| format!(" with provider '{provider}'"))
                    .unwrap_or_default(),
                required_integration
                    .kind
                    .as_deref()
                    .map(|kind| format!(" and kind '{kind}'"))
                    .unwrap_or_default(),
                required_integration
                    .integration_id
                    .as_deref()
                    .map(|id| format!(" (integration_id '{id}')"))
                    .unwrap_or_default(),
            ))
        }
    }
}

/// Inspect `args` for conventional `target_*` keys and verify they fall
/// within `scope`. Absence of a target key means the tool is not
/// targeting that axis and the check is skipped.
fn scope_violation(scope: &aura_core::AgentScope, args: &serde_json::Value) -> Option<String> {
    if scope.is_universe() {
        return None;
    }
    let obj = args.as_object()?;
    if let Some(id) = obj.get("target_org_id").and_then(|v| v.as_str()) {
        if !scope.orgs.is_empty() && !scope.orgs.iter().any(|o| o == id) {
            return Some(format!("permissions: target out of scope (org '{id}')"));
        }
    }
    if let Some(id) = obj.get("target_project_id").and_then(|v| v.as_str()) {
        if !scope.projects.is_empty() && !scope.projects.iter().any(|p| p == id) {
            return Some(format!("permissions: target out of scope (project '{id}')"));
        }
    }
    if let Some(id) = obj.get("target_agent_id").and_then(|v| v.as_str()) {
        if !scope.agent_ids.is_empty() && !scope.agent_ids.iter().any(|a| a == id) {
            return Some(format!("permissions: target out of scope (agent '{id}')"));
        }
    }
    None
}

#[cfg(test)]
mod permission_tests {
    use super::*;
    use aura_core::{AgentPermissions, AgentScope, Capability};
    use bytes::Bytes;

    fn delegate_proposal(tool: &str, args: serde_json::Value) -> Proposal {
        let call = ToolCall::new(tool, args);
        let payload = serde_json::to_vec(&call).unwrap();
        Proposal::new(ActionKind::Delegate, Bytes::from(payload))
    }

    #[test]
    fn default_empty_permissions_allow_unrestricted_tools() {
        // A tool that carries no capability requirement and no scope
        // target keys passes against the default `AgentPermissions::empty()`.
        let policy = Policy::with_defaults();
        let proposal = delegate_proposal("read_file", serde_json::json!({"path":"a.txt"}));
        assert!(policy.check(&proposal).allowed);
    }

    #[test]
    fn missing_capability_is_denied() {
        let config = PolicyConfig::default()
            .with_agent_permissions(AgentPermissions {
                scope: AgentScope::default(),
                capabilities: vec![],
            })
            .with_tool_capability("spawn_agent", Capability::SpawnAgent);
        let policy = Policy::new(config);
        let result = policy.check(&delegate_proposal("spawn_agent", serde_json::json!({})));
        assert!(!result.allowed);
        assert!(result.reason.unwrap().contains("capability"));
    }

    #[test]
    fn present_capability_is_allowed() {
        let config = PolicyConfig::default()
            .with_agent_permissions(AgentPermissions {
                scope: AgentScope::default(),
                capabilities: vec![Capability::SpawnAgent],
            })
            .with_tool_capability("spawn_agent", Capability::SpawnAgent);
        let policy = Policy::new(config);
        assert!(
            policy
                .check(&delegate_proposal("spawn_agent", serde_json::json!({})))
                .allowed
        );
    }

    #[test]
    fn out_of_scope_target_is_denied() {
        let config = PolicyConfig::default().with_agent_permissions(AgentPermissions {
            scope: AgentScope {
                orgs: vec!["only".into()],
                ..AgentScope::default()
            },
            capabilities: vec![],
        });
        let policy = Policy::new(config);
        let result = policy.check(&delegate_proposal(
            "any_tool",
            serde_json::json!({"target_org_id":"other"}),
        ));
        assert!(!result.allowed);
        assert!(result.reason.unwrap().contains("out of scope"));
    }

    #[test]
    fn in_scope_target_is_allowed() {
        let config = PolicyConfig::default().with_agent_permissions(AgentPermissions {
            scope: AgentScope {
                orgs: vec!["only".into()],
                ..AgentScope::default()
            },
            capabilities: vec![],
        });
        let policy = Policy::new(config);
        assert!(
            policy
                .check(&delegate_proposal(
                    "any_tool",
                    serde_json::json!({"target_org_id":"only"})
                ))
                .allowed
        );
    }
}
