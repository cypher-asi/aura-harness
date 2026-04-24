//! Policy engine proper — the [`Policy`] type and its authorization
//! pipeline.
//!
//! `check.rs` holds everything that actively *evaluates* a [`Proposal`] or
//! [`ToolCall`] against the declarative [`super::config::PolicyConfig`].
//! Tool-state resolution, runtime-capability checks, and
//! agent-permission scope checks all live here.

use super::config::PolicyConfig;
use crate::{PendingToolPrompt, ToolApprovalRemember};
use aura_core::{
    installed_integrations_satisfy, resolve_effective_permission, ActionKind, Proposal,
    RuntimeCapabilityInstall, ToolCall, ToolState,
};
use std::collections::HashMap;
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
    session_tool_states: Mutex<HashMap<String, ToolState>>,
}

/// Distinguishable verdict returned by the tool authorization pipeline.
///
/// Phase 6 (security audit) split what used to be "allowed / not allowed"
/// into three cases so downstream code can differentiate a permanent
/// deny from a proposal that is waiting on an out-of-band operator
/// approval. `Allow` carries no reason because "allowed" is the sole
/// happy path; the other two always carry a human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyVerdict {
    /// Allowed to proceed.
    Allow,
    /// Denied at the policy layer pending a live approval prompt.
    RequireApproval {
        /// Human-readable reason, e.g. `"Tool 'run_command' requires approval"`.
        reason: String,
        /// Structured prompt metadata for live tri-state `ask` prompts.
        prompt: Option<PendingToolPrompt>,
    },
    /// Permanently denied. No approval will unlock it.
    Deny {
        /// Human-readable reason, e.g. `"Tool 'foo' is not allowed"`.
        reason: String,
    },
}

impl PolicyVerdict {
    /// `true` iff the verdict is [`PolicyVerdict::Allow`].
    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }

    /// Extract the reason string, if any. `Allow` has none.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Allow => None,
            Self::RequireApproval { reason, .. } | Self::Deny { reason } => Some(reason.as_str()),
        }
    }
}

/// Result of policy check.
///
/// Legacy compat shim around [`PolicyVerdict`]: `allowed` is exactly
/// `verdict.is_allowed()`. Downstream code that needs to branch on
/// "approval required" vs "hard deny" should switch to [`PolicyVerdict`]
/// directly via the `*_verdict` variants of the `Policy` methods.
#[derive(Debug, Clone)]
pub struct PolicyResult {
    /// Whether the proposal is allowed.
    pub allowed: bool,
    /// Reason for rejection (if not allowed).
    pub reason: Option<String>,
    /// Structured verdict this `PolicyResult` was derived from. Phase 6
    /// additions (e.g. `process_tool_proposal`) should match on this
    /// instead of `allowed`.
    pub verdict: PolicyVerdict,
}

impl From<PolicyVerdict> for PolicyResult {
    fn from(verdict: PolicyVerdict) -> Self {
        let allowed = verdict.is_allowed();
        let reason = verdict.reason().map(std::string::ToString::to_string);
        Self {
            allowed,
            reason,
            verdict,
        }
    }
}

impl Policy {
    /// Create a new policy with the given config.
    #[must_use]
    pub fn new(config: PolicyConfig) -> Self {
        Self {
            config,
            session_tool_states: Mutex::new(HashMap::new()),
        }
    }

    /// Create a policy with default config.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(PolicyConfig::default())
    }

    /// Resolve the tri-state `on` / `off` / `ask` [`ToolState`] for
    /// `tool` against the two-level permission model (user default +
    /// per-agent override). This is the single resolution helper the
    /// kernel gate consults for per-tool enablement.
    #[must_use]
    pub fn resolve_tool_state(&self, tool: &str) -> ToolState {
        resolve_effective_permission(
            &self.config.user_default,
            self.config.agent_override.as_ref(),
            tool,
        )
    }

    /// Cache a live approval decision for this policy's current session.
    pub fn remember_tool_state_for_session(&self, tool: &str, state: ToolState) {
        self.session_tool_states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(tool.to_string(), state);
    }

    /// Clear all session approvals.
    ///
    /// Recovers gracefully from mutex poisoning by accessing the inner data.
    pub fn clear_session_approvals(&self) {
        self.session_tool_states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Return the live prompt verdict for the additive tri-state `ask`
    /// layer. `None` means the new layer has no opinion and the legacy
    /// verdict remains authoritative for Phase C.
    #[must_use]
    pub fn live_tool_prompt_verdict(
        &self,
        tool: &str,
        args: &serde_json::Value,
        agent_id: aura_core::AgentId,
        request_id: String,
        has_live_session: bool,
        remember_options: Vec<ToolApprovalRemember>,
    ) -> Option<PolicyVerdict> {
        if let Some(state) = self
            .session_tool_states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(tool)
            .copied()
        {
            return match state {
                ToolState::Allow => None,
                ToolState::Deny => Some(PolicyVerdict::Deny {
                    reason: format!("Tool '{tool}' was denied for this session"),
                }),
                ToolState::Ask => None,
            };
        }

        if self.resolve_tool_state(tool) != ToolState::Ask {
            return None;
        }

        if !has_live_session {
            return Some(PolicyVerdict::Deny {
                reason: format!("tool {tool} is set to ask; no session to prompt"),
            });
        }

        Some(PolicyVerdict::RequireApproval {
            reason: format!("Tool '{tool}' is set to ask"),
            prompt: Some(PendingToolPrompt {
                request_id,
                tool_name: tool.to_string(),
                args: args.clone(),
                agent_id,
                remember_options,
            }),
        })
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
        self.check_with_runtime_capabilities_verdict(proposal, runtime_capabilities)
            .into()
    }

    /// [`Self::check_with_runtime_capabilities`] returning the richer
    /// [`PolicyVerdict`]. Prefer this in new code so
    /// "needs operator approval" is distinguishable from "hard deny".
    #[must_use]
    pub fn check_with_runtime_capabilities_verdict(
        &self,
        proposal: &Proposal,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
    ) -> PolicyVerdict {
        if !self
            .config
            .allowed_action_kinds
            .contains(&proposal.action_kind)
        {
            warn!(kind = ?proposal.action_kind, "Action kind not allowed");
            return PolicyVerdict::Deny {
                reason: format!("Action kind {:?} not allowed", proposal.action_kind),
            };
        }

        if proposal.action_kind == ActionKind::Delegate {
            match serde_json::from_slice::<ToolCall>(&proposal.payload) {
                Ok(tool_call) => {
                    if let Some(result) = self.check_agent_permissions(&tool_call) {
                        if !result.allowed {
                            return PolicyVerdict::Deny {
                                reason: result
                                    .reason
                                    .unwrap_or_else(|| "Policy denied".to_string()),
                            };
                        }
                    }

                    let verdict = self.check_tool_with_runtime_capabilities_verdict(
                        &tool_call.tool,
                        &tool_call.args,
                        runtime_capabilities,
                    );
                    if !verdict.is_allowed() {
                        return verdict;
                    }
                }
                Err(_) => {
                    warn!("Malformed delegate payload");
                    return PolicyVerdict::Deny {
                        reason: "Malformed delegate payload".to_string(),
                    };
                }
            }
        }

        debug!(kind = ?proposal.action_kind, "Proposal allowed");
        PolicyVerdict::Allow
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
        input: &serde_json::Value,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
    ) -> PolicyResult {
        self.check_tool_with_runtime_capabilities_verdict(tool, input, runtime_capabilities)
            .into()
    }

    /// [`Self::check_tool_with_runtime_capabilities`] returning the
    /// structured [`PolicyVerdict`] instead of the compat shim.
    #[must_use]
    pub fn check_tool_with_runtime_capabilities_verdict(
        &self,
        tool: &str,
        _input: &serde_json::Value,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
    ) -> PolicyVerdict {
        if let Some(reason) = self.integration_requirement_satisfied(tool, runtime_capabilities) {
            return PolicyVerdict::Deny { reason };
        }

        if let Some(state) = self
            .session_tool_states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(tool)
            .copied()
        {
            return match state {
                ToolState::Allow => PolicyVerdict::Allow,
                ToolState::Deny => PolicyVerdict::Deny {
                    reason: format!("Tool '{tool}' was denied for this session"),
                },
                ToolState::Ask => PolicyVerdict::RequireApproval {
                    reason: format!("Tool '{tool}' is set to ask"),
                    prompt: None,
                },
            };
        }

        match self.resolve_tool_state(tool) {
            ToolState::Deny => PolicyVerdict::Deny {
                reason: format!("Tool '{tool}' is not allowed"),
            },
            ToolState::Allow => PolicyVerdict::Allow,
            ToolState::Ask => PolicyVerdict::RequireApproval {
                reason: format!("Tool '{tool}' is set to ask"),
                prompt: None,
            },
        }
    }

    /// Get maximum allowed proposals.
    #[must_use]
    pub const fn max_proposals(&self) -> usize {
        self.config.max_proposals
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
                return Some(
                    PolicyVerdict::Deny {
                        reason: format!("permissions: requires capability {required:?}"),
                    }
                    .into(),
                );
            }
        }

        if let Some(reason) = scope_violation(&permissions.scope, &tool_call.args) {
            return Some(PolicyVerdict::Deny { reason }.into());
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
                installed_integrations_satisfy(
                    required_integration,
                    &self.config.installed_integrations,
                )
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
    fn resolve_tool_state_default_is_on() {
        let policy = Policy::with_defaults();
        assert_eq!(policy.resolve_tool_state("read_file"), ToolState::Allow);
        assert_eq!(policy.resolve_tool_state("run_command"), ToolState::Allow);
        assert_eq!(policy.resolve_tool_state("anything"), ToolState::Allow);
    }

    #[test]
    fn resolve_tool_state_auto_review_is_ask_for_everything() {
        let cfg =
            PolicyConfig::default().with_user_default(aura_core::UserToolDefaults::auto_review());
        let policy = Policy::new(cfg);
        assert_eq!(policy.resolve_tool_state("read_file"), ToolState::Ask);
        assert_eq!(policy.resolve_tool_state("run_command"), ToolState::Ask);
    }

    #[test]
    fn resolve_tool_state_default_permissions_mode_is_tri_state_per_tool() {
        let mut per_tool = std::collections::BTreeMap::new();
        per_tool.insert("read_file".into(), ToolState::Allow);
        per_tool.insert("run_command".into(), ToolState::Ask);
        per_tool.insert("delete_file".into(), ToolState::Deny);
        let user_default =
            aura_core::UserToolDefaults::default_permissions(per_tool, ToolState::Deny);
        let cfg = PolicyConfig::default().with_user_default(user_default);
        let policy = Policy::new(cfg);
        assert_eq!(policy.resolve_tool_state("read_file"), ToolState::Allow);
        assert_eq!(policy.resolve_tool_state("run_command"), ToolState::Ask);
        assert_eq!(policy.resolve_tool_state("delete_file"), ToolState::Deny);
        assert_eq!(
            policy.resolve_tool_state("not_in_map"),
            ToolState::Deny,
            "fallback applies to unlisted tools",
        );
    }

    #[test]
    fn resolve_tool_state_agent_override_wins_over_user_default() {
        let cfg = PolicyConfig::default()
            .with_user_default(aura_core::UserToolDefaults::full_access())
            .with_agent_override(Some(
                aura_core::AgentToolPermissions::new()
                    .with("run_command", ToolState::Deny)
                    .with("delete_file", ToolState::Ask),
            ));
        let policy = Policy::new(cfg);
        assert_eq!(
            policy.resolve_tool_state("run_command"),
            ToolState::Deny,
            "override flips user's full_access to off",
        );
        assert_eq!(
            policy.resolve_tool_state("delete_file"),
            ToolState::Ask,
            "override flips user's full_access to ask",
        );
        assert_eq!(
            policy.resolve_tool_state("read_file"),
            ToolState::Allow,
            "unlisted tool still flows through user default (on)",
        );
    }

    #[test]
    fn live_prompt_verdict_denies_ask_without_session() {
        let cfg =
            PolicyConfig::default().with_user_default(aura_core::UserToolDefaults::auto_review());
        let policy = Policy::new(cfg);
        let verdict = policy
            .live_tool_prompt_verdict(
                "read_file",
                &serde_json::json!({"path": "a.txt"}),
                aura_core::AgentId::generate(),
                "request-1".to_string(),
                false,
                vec![ToolApprovalRemember::Once],
            )
            .expect("ask state should produce a verdict");

        assert!(
            matches!(verdict, PolicyVerdict::Deny { ref reason } if reason.contains("no session to prompt"))
        );
    }

    #[test]
    fn live_prompt_verdict_carries_structured_prompt() {
        let cfg =
            PolicyConfig::default().with_user_default(aura_core::UserToolDefaults::auto_review());
        let policy = Policy::new(cfg);
        let agent_id = aura_core::AgentId::generate();
        let verdict = policy
            .live_tool_prompt_verdict(
                "read_file",
                &serde_json::json!({"path": "a.txt"}),
                agent_id,
                "request-1".to_string(),
                true,
                vec![ToolApprovalRemember::Once, ToolApprovalRemember::Session],
            )
            .expect("ask state should produce a verdict");

        match verdict {
            PolicyVerdict::RequireApproval {
                prompt: Some(prompt),
                ..
            } => {
                assert_eq!(prompt.request_id, "request-1");
                assert_eq!(prompt.tool_name, "read_file");
                assert_eq!(prompt.args, serde_json::json!({"path": "a.txt"}));
                assert_eq!(prompt.agent_id, agent_id);
                assert_eq!(prompt.remember_options.len(), 2);
            }
            other => panic!("expected structured prompt, got {other:?}"),
        }
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
