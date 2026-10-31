//! Policy engine proper — the [`Policy`] type and its authorization
//! pipeline.
//!
//! `check` holds everything that actively *evaluates* a [`Proposal`] or
//! [`ToolCall`] against the declarative [`super::config::PolicyConfig`].
//! The pipeline is split across siblings:
//!
//! - [`verdict`] — `PolicyVerdict` + `PolicyResult` shim types.
//! - [`agent_permissions`] — `Policy::resolve_tool_state` and the
//!   capability + scope check on a `ToolCall`.
//! - [`integration_gate`] — the integration-requirement check that
//!   delegates to `aura_core_types::installed_integrations_satisfy`.
//! - [`scope`] — `target_*` scope-key validation.
//! - [`mode_gate`] — the session `AgentMode` ceiling, checked first.
//! - [`delegate_gate`] — `ActionKind::Delegate` payload parsing and
//!   evaluation against the mode, per-agent, and tool gates.

mod agent_permissions;
mod delegate_gate;
mod integration_gate;
mod mode_gate;
mod scope;
#[cfg(test)]
mod tests;
mod verdict;

pub use mode_gate::{gated_action, tool_mode_class, ToolModeClass};
pub use verdict::{PolicyEvaluation, PolicyResult, PolicyVerdict};

use super::config::PolicyConfig;
use crate::{PendingToolPrompt, ToolApprovalRemember};
use aura_core_types::{
    ActionKind, PermissionLayer, PolicyCheck, PolicyCheckOutcome, PolicyRule, PolicySource,
    Proposal, RuntimeCapabilityInstall, ToolState,
};
use std::collections::HashMap;
use std::sync::Mutex;
use tracing::{debug, warn};
use verdict::outcome_of;

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
    pub(super) config: PolicyConfig,
    session_tool_states: Mutex<HashMap<String, ToolState>>,
    snapshot_hash: String,
}

impl Policy {
    /// Create a new policy with the given config.
    #[must_use]
    pub fn new(config: PolicyConfig) -> Self {
        let snapshot_hash = config.snapshot_hash();
        Self {
            config,
            session_tool_states: Mutex::new(HashMap::new()),
            snapshot_hash,
        }
    }

    /// Create a policy with default config.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(PolicyConfig::default())
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
        agent_id: aura_core_types::AgentId,
        request_id: String,
        has_live_session: bool,
        remember_options: Vec<ToolApprovalRemember>,
    ) -> Option<PolicyVerdict> {
        self.live_tool_prompt_explained(
            tool,
            args,
            agent_id,
            request_id,
            has_live_session,
            remember_options,
        )
        .map(|(verdict, _)| verdict)
    }

    /// [`Self::live_tool_prompt_verdict`] plus the rule that produced the
    /// verdict.
    #[must_use]
    pub fn live_tool_prompt_explained(
        &self,
        tool: &str,
        args: &serde_json::Value,
        agent_id: aura_core_types::AgentId,
        request_id: String,
        has_live_session: bool,
        remember_options: Vec<ToolApprovalRemember>,
    ) -> Option<(PolicyVerdict, PolicyRule)> {
        if let Some(state) = self.session_tool_state(tool) {
            return match state {
                ToolState::Allow | ToolState::Ask => None,
                ToolState::Deny => Some((
                    PolicyVerdict::Deny {
                        reason: format!("Tool '{tool}' was denied for this session"),
                    },
                    session_rule(tool, state),
                )),
            };
        }

        let (state, layer) = self.resolve_tool_state_with_source(tool);
        if state != ToolState::Ask {
            return None;
        }

        if !has_live_session {
            return Some((
                PolicyVerdict::Deny {
                    reason: format!("tool {tool} is set to ask; no session to prompt"),
                },
                PolicyRule::new(PolicySource::NoLiveSession, format!("{tool}=ask")),
            ));
        }

        Some((
            PolicyVerdict::RequireApproval {
                reason: format!("Tool '{tool}' is set to ask"),
                prompt: Some(PendingToolPrompt {
                    request_id,
                    tool_name: tool.to_string(),
                    args: args.clone(),
                    agent_id,
                    remember_options,
                }),
            },
            permission_rule(tool, state, layer),
        ))
    }

    fn session_tool_state(&self, tool: &str) -> Option<ToolState> {
        self.session_tool_states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(tool)
            .copied()
    }

    /// Evaluate a proposal through the full authorization pipeline,
    /// recording every gate and the rule that produced the verdict.
    ///
    /// [`Self::check_with_runtime_capabilities_verdict`] is a thin
    /// wrapper over this, so the recorded provenance can never drift from
    /// the enforced verdict.
    #[must_use]
    pub fn evaluate_proposal(
        &self,
        proposal: &Proposal,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
    ) -> PolicyEvaluation {
        let mut trace = Vec::new();
        let kind_rule = PolicyRule::new(
            PolicySource::ActionKind,
            format!("{:?}", proposal.action_kind).to_ascii_lowercase(),
        );
        if !self
            .config
            .allowed_action_kinds
            .contains(&proposal.action_kind)
        {
            warn!(kind = ?proposal.action_kind, "Action kind not allowed");
            trace.push(PolicyCheck {
                rule: kind_rule.clone(),
                outcome: PolicyCheckOutcome::Deny,
            });
            return PolicyEvaluation::new(
                PolicyVerdict::Deny {
                    reason: format!("Action kind {:?} not allowed", proposal.action_kind),
                },
                kind_rule,
                trace,
            );
        }
        trace.push(PolicyCheck {
            rule: kind_rule.clone(),
            outcome: PolicyCheckOutcome::Allow,
        });

        if proposal.action_kind == ActionKind::Delegate {
            let (verdict, rule) = self.evaluate_delegate_explained(
                &proposal.payload,
                runtime_capabilities,
                &mut trace,
            );
            if !verdict.is_allowed() {
                return PolicyEvaluation::new(verdict, rule, trace);
            }
            debug!(kind = ?proposal.action_kind, "Proposal allowed");
            return PolicyEvaluation::new(PolicyVerdict::Allow, rule, trace);
        }

        debug!(kind = ?proposal.action_kind, "Proposal allowed");
        PolicyEvaluation::new(PolicyVerdict::Allow, kind_rule, trace)
    }

    /// Integration, session-grant, and tri-state tool gate with
    /// provenance. Shared by the delegate pipeline and
    /// [`Self::check_tool_with_runtime_capabilities_verdict`].
    pub(super) fn tool_gate_explained(
        &self,
        tool: &str,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
        trace: &mut Vec<PolicyCheck>,
    ) -> (PolicyVerdict, PolicyRule) {
        let integration_rule = PolicyRule::new(PolicySource::Integration, "requirements");
        if let Some(reason) = self.integration_requirement_satisfied(tool, runtime_capabilities) {
            trace.push(PolicyCheck {
                rule: integration_rule.clone(),
                outcome: PolicyCheckOutcome::Deny,
            });
            return (PolicyVerdict::Deny { reason }, integration_rule);
        }
        trace.push(PolicyCheck {
            rule: integration_rule,
            outcome: PolicyCheckOutcome::Allow,
        });

        let (verdict, rule) = if let Some(state) = self.session_tool_state(tool) {
            let verdict = match state {
                ToolState::Allow => PolicyVerdict::Allow,
                ToolState::Deny => PolicyVerdict::Deny {
                    reason: format!("Tool '{tool}' was denied for this session"),
                },
                ToolState::Ask => PolicyVerdict::RequireApproval {
                    reason: format!("Tool '{tool}' is set to ask"),
                    prompt: None,
                },
            };
            (verdict, session_rule(tool, state))
        } else {
            let (state, layer) = self.resolve_tool_state_with_source(tool);
            let verdict = match state {
                ToolState::Deny => PolicyVerdict::Deny {
                    reason: format!("Tool '{tool}' is not allowed"),
                },
                ToolState::Allow => PolicyVerdict::Allow,
                ToolState::Ask => PolicyVerdict::RequireApproval {
                    reason: format!("Tool '{tool}' is set to ask"),
                    prompt: None,
                },
            };
            (verdict, permission_rule(tool, state, layer))
        };
        trace.push(PolicyCheck {
            rule: rule.clone(),
            outcome: outcome_of(&verdict),
        });
        (verdict, rule)
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
        self.evaluate_proposal(proposal, runtime_capabilities)
            .verdict
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
        self.tool_gate_explained(tool, runtime_capabilities, &mut Vec::new())
            .0
    }

    /// Hash of the policy configuration snapshot, recorded on every
    /// `PolicyDecision` so auditors can tell which configuration decided.
    #[must_use]
    pub fn snapshot_hash(&self) -> &str {
        &self.snapshot_hash
    }

    /// Lower-case label of the session mode this policy enforces.
    #[must_use]
    pub fn mode_label(&self) -> String {
        delegate_gate::mode_label(self.config.mode)
    }

    /// Get maximum allowed proposals.
    #[must_use]
    pub const fn max_proposals(&self) -> usize {
        self.config.max_proposals
    }
}

/// Wire label of a tri-state, matching its serde form (`on`/`off`/`ask`).
pub(crate) const fn tool_state_label(state: ToolState) -> &'static str {
    match state {
        ToolState::Allow => "on",
        ToolState::Deny => "off",
        ToolState::Ask => "ask",
    }
}

fn session_rule(tool: &str, state: ToolState) -> PolicyRule {
    PolicyRule::new(
        PolicySource::SessionGrant,
        format!("{tool}={}", tool_state_label(state)),
    )
}

fn permission_rule(tool: &str, state: ToolState, layer: PermissionLayer) -> PolicyRule {
    let state = tool_state_label(state);
    match layer {
        PermissionLayer::AgentOverride => PolicyRule::new(
            PolicySource::AgentOverride,
            format!("per_tool[{tool}]={state}"),
        ),
        PermissionLayer::UserFullAccess => {
            PolicyRule::new(PolicySource::UserDefault, "full_access")
        }
        PermissionLayer::UserAutoReview => {
            PolicyRule::new(PolicySource::UserDefault, "auto_review")
        }
        PermissionLayer::UserPerTool => PolicyRule::new(
            PolicySource::UserDefault,
            format!("per_tool[{tool}]={state}"),
        ),
        PermissionLayer::UserFallback => {
            PolicyRule::new(PolicySource::UserDefault, format!("fallback={state}"))
        }
    }
}
