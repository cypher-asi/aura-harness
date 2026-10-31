//! [`ActionKind::Delegate`] proposal parsing and evaluation.
//!
//! A `Delegate` proposal carries a serialized [`ToolCall`] in its
//! payload. The delegate gate parses the payload, runs the session mode
//! gate, then the per-agent permission check, then defers to the tool /
//! runtime-capability checks for the final verdict.

use super::verdict::{outcome_of, PolicyVerdict};
use super::Policy;
use aura_core_types::{
    PolicyCheck, PolicyCheckOutcome, PolicyRule, PolicySource, RuntimeCapabilityInstall, ToolCall,
};
use tracing::warn;

impl Policy {
    /// Evaluate a `Delegate` proposal payload, recording every gate in
    /// `trace` and returning the verdict together with the rule that
    /// produced it.
    pub(super) fn evaluate_delegate_explained(
        &self,
        payload: &[u8],
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
        trace: &mut Vec<PolicyCheck>,
    ) -> (PolicyVerdict, PolicyRule) {
        let Ok(tool_call) = serde_json::from_slice::<ToolCall>(payload) else {
            warn!("Malformed delegate payload");
            let rule = PolicyRule::new(PolicySource::MalformedProposal, "delegate_payload");
            trace.push(PolicyCheck {
                rule: rule.clone(),
                outcome: PolicyCheckOutcome::Deny,
            });
            return (
                PolicyVerdict::Deny {
                    reason: "Malformed delegate payload".to_string(),
                },
                rule,
            );
        };

        let mode_rule = PolicyRule::new(PolicySource::Mode, mode_label(self.config.mode));
        if let Some(verdict) = self.check_mode(&tool_call) {
            trace.push(PolicyCheck {
                rule: mode_rule.clone(),
                outcome: outcome_of(&verdict),
            });
            return (verdict, mode_rule);
        }
        trace.push(PolicyCheck {
            rule: mode_rule,
            outcome: PolicyCheckOutcome::Allow,
        });

        let capability_rule = PolicyRule::new(
            PolicySource::AgentCapability,
            self.config
                .tool_capability_requirements
                .get(&tool_call.tool)
                .map_or_else(
                    || "no_requirement".to_string(),
                    |cap| format!("requires {cap:?}"),
                ),
        );
        if let Some(result) = self.check_agent_permissions(&tool_call) {
            if !result.allowed {
                trace.push(PolicyCheck {
                    rule: capability_rule.clone(),
                    outcome: PolicyCheckOutcome::Deny,
                });
                return (
                    PolicyVerdict::Deny {
                        reason: result.reason.unwrap_or_else(|| "Policy denied".to_string()),
                    },
                    capability_rule,
                );
            }
        }
        trace.push(PolicyCheck {
            rule: capability_rule,
            outcome: PolicyCheckOutcome::Allow,
        });

        self.tool_gate_explained(&tool_call.tool, runtime_capabilities, trace)
    }
}

/// Lower-case label for a session mode (`agent`, `plan`, `ask`, `debug`).
pub(crate) fn mode_label(mode: aura_core_modes::AgentMode) -> String {
    format!("{mode:?}").to_ascii_lowercase()
}
