//! Recorded policy decisions for tool proposals.
//!
//! A [`PolicyDecision`] is the payload of a
//! [`crate::TransactionType::PolicyDecision`] record entry. It sits between
//! the `ToolProposal` entry it references and the `ToolExecution` entry that
//! follows when the call was approved, and names the exact policy rule that
//! produced the verdict so an auditor can answer "what allowed this call".

use super::proposal::ToolGateVerdict;
use serde::{Deserialize, Serialize};

/// Which policy layer produced a check outcome or the final verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicySource {
    /// Allowed action-kind set on the policy config.
    ActionKind,
    /// Session `AgentMode` ceiling.
    Mode,
    /// Per-agent capability + scope check.
    AgentCapability,
    /// Installed-integration requirement for the tool.
    Integration,
    /// Live approval cached for the current session.
    SessionGrant,
    /// Per-agent tool permission override.
    AgentOverride,
    /// User-wide tool defaults.
    UserDefault,
    /// `PermissionRequest` plugin hook.
    PermissionHook,
    /// Interactive user approval.
    User,
    /// Tool is set to `ask` but no live session can be prompted.
    NoLiveSession,
    /// Tool payload could not be parsed into a `ToolCall`.
    MalformedProposal,
}

impl PolicySource {
    /// Stable snake_case label, identical to the serde wire form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ActionKind => "action_kind",
            Self::Mode => "mode",
            Self::AgentCapability => "agent_capability",
            Self::Integration => "integration",
            Self::SessionGrant => "session_grant",
            Self::AgentOverride => "agent_override",
            Self::UserDefault => "user_default",
            Self::PermissionHook => "permission_hook",
            Self::User => "user",
            Self::NoLiveSession => "no_live_session",
            Self::MalformedProposal => "malformed_proposal",
        }
    }
}

/// A specific policy rule: the layer plus a human-readable rule key,
/// e.g. `user_default` + `per_tool[run_command]=allow`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRule {
    /// Layer the rule belongs to.
    pub source: PolicySource,
    /// Rule key within the layer.
    pub rule: String,
}

impl PolicyRule {
    /// Construct a rule.
    #[must_use]
    pub fn new(source: PolicySource, rule: impl Into<String>) -> Self {
        Self {
            source,
            rule: rule.into(),
        }
    }
}

impl std::fmt::Display for PolicyRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.rule.is_empty() {
            f.write_str(self.source.as_str())
        } else {
            write!(f, "{}.{}", self.source.as_str(), self.rule)
        }
    }
}

/// Outcome of a single policy check in the evaluation trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyCheckOutcome {
    /// The check passed (or allowed the call outright).
    Allow,
    /// The check denied the call.
    Deny,
    /// The check requires a live approval.
    Ask,
}

/// One evaluated gate in the order the policy ran it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyCheck {
    /// The rule that was evaluated.
    #[serde(flatten)]
    pub rule: PolicyRule,
    /// What the rule decided.
    pub outcome: PolicyCheckOutcome,
}

/// Who answered a live approval request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PolicyApprover {
    /// A `PermissionRequest` plugin hook short-circuited the prompt.
    Hook,
    /// An interactive user answered the prompt.
    User {
        /// Originating user id, when the session knows it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_id: Option<String>,
    },
}

/// Details of a live approval that contributed to the decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyApproval {
    /// Who answered.
    pub approver: PolicyApprover,
    /// Remember scope chosen by the approver (`once`, `session`, `forever`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remember: Option<String>,
    /// Prompt request id, when a prompt was issued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Payload of a `PolicyDecision` transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    /// `tool_use_id` of the proposal this decision covers.
    pub tool_use_id: String,
    /// Tool name.
    pub tool: String,
    /// Final verdict.
    pub verdict: ToolGateVerdict,
    /// Human-readable reason (always set for denials).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The rule that produced the final verdict.
    pub decided_by: PolicyRule,
    /// Every gate evaluated, in evaluation order.
    #[serde(default)]
    pub trace: Vec<PolicyCheck>,
    /// Live approval details, when the ask path ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<PolicyApproval>,
    /// Session mode the policy evaluated under.
    pub mode: String,
    /// BLAKE3 hex digest of the policy configuration snapshot used.
    pub policy_hash: String,
    /// Hex action id accepted for execution (approved decisions only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_decision_roundtrip_and_wire_shape() {
        let decision = PolicyDecision {
            tool_use_id: "tu_1".into(),
            tool: "run_command".into(),
            verdict: ToolGateVerdict::Approved,
            reason: None,
            decided_by: PolicyRule::new(PolicySource::User, "approved"),
            trace: vec![
                PolicyCheck {
                    rule: PolicyRule::new(PolicySource::Mode, "agent"),
                    outcome: PolicyCheckOutcome::Allow,
                },
                PolicyCheck {
                    rule: PolicyRule::new(PolicySource::UserDefault, "auto_review"),
                    outcome: PolicyCheckOutcome::Ask,
                },
            ],
            approval: Some(PolicyApproval {
                approver: PolicyApprover::User {
                    user_id: Some("u1".into()),
                },
                remember: Some("session".into()),
                request_id: Some("req".into()),
            }),
            mode: "agent".into(),
            policy_hash: "ab".into(),
            action_id: Some("00".into()),
        };
        let json = serde_json::to_value(&decision).unwrap();
        assert_eq!(json["decided_by"]["source"], "user");
        assert_eq!(json["trace"][1]["source"], "user_default");
        assert_eq!(json["trace"][1]["outcome"], "ask");
        assert_eq!(json["approval"]["approver"]["kind"], "user");
        let parsed: PolicyDecision = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, decision);
    }

    #[test]
    fn policy_rule_display() {
        let rule = PolicyRule::new(PolicySource::AgentOverride, "per_tool[run_command]=allow");
        assert_eq!(
            rule.to_string(),
            "agent_override.per_tool[run_command]=allow"
        );
        assert_eq!(
            PolicyRule::new(PolicySource::SessionGrant, "").to_string(),
            "session_grant"
        );
    }
}
