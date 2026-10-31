//! Wire ↔ core conversion helpers.
//!
//! Moved from `aura-runtime::protocol` in Phase A of the gateway
//! refactor so `aura-protocol` is the single canonical wire↔core
//! seam.
//!
//! The functions here turn the wire-shape mirrors carried on a
//! [`crate::RuntimeRequest`] into the `aura-core` types every
//! harness-internal consumer (kernel policy, tool resolver, executor
//! router) actually speaks. Both crates sit in the `core` layer so
//! this is a same-layer dependency edge.

use aura_core_types::{
    AgentToolPermissions, InstalledIntegrationDefinition, InstalledToolDefinition, PolicyApprover,
    PolicyDecision, RecordEntry, ToolState, TransactionType,
};

use crate::common::ToolStateWire;
use crate::installed::{
    InstalledIntegration, InstalledTool, InstalledToolRuntimeAuth, InstalledToolRuntimeExecution,
    ToolAuth,
};
use crate::permissions::AgentToolPermissionsWire;
use crate::server::{PolicyCheckWire, PolicyRuleWire, ToolPolicyDecision};

/// Convert a tri-state tool permission wire value into the
/// in-process [`ToolState`].
#[must_use]
pub fn tool_state_from_wire(state: ToolStateWire) -> ToolState {
    match state {
        ToolStateWire::On => ToolState::Allow,
        ToolStateWire::Off => ToolState::Deny,
        ToolStateWire::Ask => ToolState::Ask,
    }
}

/// Inverse of [`tool_state_from_wire`] — convert an in-process
/// [`ToolState`] back into its [`ToolStateWire`] mirror.
#[must_use]
pub fn tool_state_to_wire(state: ToolState) -> ToolStateWire {
    match state {
        ToolState::Allow => ToolStateWire::On,
        ToolState::Deny => ToolStateWire::Off,
        ToolState::Ask => ToolStateWire::Ask,
    }
}

/// Convert wire-side [`AgentToolPermissionsWire`] into the
/// harness-core [`AgentToolPermissions`].
#[must_use]
pub fn agent_tool_permissions_from_wire(wire: AgentToolPermissionsWire) -> AgentToolPermissions {
    AgentToolPermissions {
        per_tool: wire
            .per_tool
            .into_iter()
            .map(|(name, state)| (name, tool_state_from_wire(state)))
            .collect(),
    }
}

/// Convert a protocol [`InstalledTool`] into a core
/// [`InstalledToolDefinition`].
#[must_use]
pub fn installed_tool_to_core(t: InstalledTool) -> InstalledToolDefinition {
    InstalledToolDefinition {
        name: t.name,
        description: t.description,
        input_schema: t.input_schema,
        endpoint: t.endpoint,
        auth: match t.auth {
            ToolAuth::None => aura_core_types::ToolAuth::None,
            ToolAuth::Bearer { token } => aura_core_types::ToolAuth::Bearer { token },
            ToolAuth::ApiKey { header, key } => aura_core_types::ToolAuth::ApiKey { header, key },
            ToolAuth::Headers { headers } => aura_core_types::ToolAuth::Headers { headers },
        },
        timeout_ms: t.timeout_ms,
        namespace: t.namespace,
        required_integration: t.required_integration.map(|requirement| {
            aura_core_types::InstalledToolIntegrationRequirement {
                integration_id: requirement.integration_id,
                provider: requirement.provider,
                kind: requirement.kind,
            }
        }),
        runtime_execution: t.runtime_execution.map(|execution| match execution {
            InstalledToolRuntimeExecution::AppProvider(provider) => {
                aura_core_types::InstalledToolRuntimeExecution::AppProvider(
                    aura_core_types::InstalledToolRuntimeProviderExecution {
                        provider: provider.provider,
                        base_url: provider.base_url,
                        static_headers: provider.static_headers,
                        integrations: provider
                            .integrations
                            .into_iter()
                            .map(|integration| aura_core_types::InstalledToolRuntimeIntegration {
                                integration_id: integration.integration_id,
                                base_url: integration.base_url,
                                auth: match integration.auth {
                                    InstalledToolRuntimeAuth::None => {
                                        aura_core_types::InstalledToolRuntimeAuth::None
                                    }
                                    InstalledToolRuntimeAuth::AuthorizationBearer { token } => {
                                        aura_core_types::InstalledToolRuntimeAuth::AuthorizationBearer {
                                            token,
                                        }
                                    }
                                    InstalledToolRuntimeAuth::AuthorizationRaw { value } => {
                                        aura_core_types::InstalledToolRuntimeAuth::AuthorizationRaw {
                                            value,
                                        }
                                    }
                                    InstalledToolRuntimeAuth::Header { name, value } => {
                                        aura_core_types::InstalledToolRuntimeAuth::Header { name, value }
                                    }
                                    InstalledToolRuntimeAuth::QueryParam { name, value } => {
                                        aura_core_types::InstalledToolRuntimeAuth::QueryParam {
                                            name,
                                            value,
                                        }
                                    }
                                    InstalledToolRuntimeAuth::Basic { username, password } => {
                                        aura_core_types::InstalledToolRuntimeAuth::Basic {
                                            username,
                                            password,
                                        }
                                    }
                                },
                                provider_config: integration.provider_config,
                            })
                            .collect(),
                    },
                )
            }
        }),
        metadata: t.metadata,
    }
}

/// Build the `tool_policy_decision` wire message from a recorded
/// `PolicyDecision` record entry. Returns `None` for any other entry.
#[must_use]
pub fn tool_policy_decision_from_entry(entry: &RecordEntry) -> Option<ToolPolicyDecision> {
    if entry.tx.tx_type != TransactionType::PolicyDecision {
        return None;
    }
    let decision: PolicyDecision = serde_json::from_slice(&entry.tx.payload).ok()?;
    let (approved_by, remember) = decision.approval.map_or((None, None), |approval| {
        let approver = match approval.approver {
            PolicyApprover::Hook => "hook",
            PolicyApprover::User { .. } => "user",
        };
        (Some(approver.to_string()), approval.remember)
    });
    Some(ToolPolicyDecision {
        tool_use_id: decision.tool_use_id,
        tool_name: decision.tool,
        verdict: serde_label(&decision.verdict),
        reason: decision.reason,
        decided_by: PolicyRuleWire {
            source: decision.decided_by.source.as_str().to_string(),
            rule: decision.decided_by.rule,
        },
        trace: decision
            .trace
            .into_iter()
            .map(|check| PolicyCheckWire {
                source: check.rule.source.as_str().to_string(),
                rule: check.rule.rule,
                outcome: serde_label(&check.outcome),
            })
            .collect(),
        approved_by,
        remember,
        mode: decision.mode,
        policy_hash: decision.policy_hash,
        agent_id: entry.tx.agent_id.to_hex(),
        seq: entry.seq,
        tx_hash: entry.tx.hash.to_hex(),
        proposal_tx_hash: entry
            .tx
            .reference_tx_hash
            .map(|hash| hash.to_hex())
            .unwrap_or_default(),
    })
}

/// The serde string form of a unit enum variant (e.g. `"pending_approval"`).
fn serde_label<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Convert a protocol [`InstalledIntegration`] into a core
/// [`InstalledIntegrationDefinition`].
#[must_use]
pub fn installed_integration_to_core(
    integration: InstalledIntegration,
) -> InstalledIntegrationDefinition {
    InstalledIntegrationDefinition {
        integration_id: integration.integration_id,
        name: integration.name,
        provider: integration.provider,
        kind: integration.kind,
        metadata: integration.metadata,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::{
        AgentId, Hash, PolicyApproval, PolicyCheck, PolicyCheckOutcome, PolicyRule, PolicySource,
        ToolGateVerdict, Transaction,
    };

    #[test]
    fn tool_policy_decision_wire_carries_rule_trace_and_links() {
        let agent_id = AgentId::generate();
        let proposal_hash = Hash::from_content(b"proposal");
        let decision = PolicyDecision {
            tool_use_id: "tu-1".into(),
            tool: "run_command".into(),
            verdict: ToolGateVerdict::PendingApproval,
            reason: Some("Tool 'run_command' is set to ask".into()),
            decided_by: PolicyRule::new(PolicySource::User, "ask"),
            trace: vec![PolicyCheck {
                rule: PolicyRule::new(PolicySource::UserDefault, "auto_review"),
                outcome: PolicyCheckOutcome::Ask,
            }],
            approval: Some(PolicyApproval {
                approver: PolicyApprover::User { user_id: None },
                remember: Some("once".into()),
                request_id: None,
            }),
            mode: "agent".into(),
            policy_hash: "abc".into(),
            action_id: None,
        };
        let tx = Transaction::policy_decision(agent_id, &decision, proposal_hash).unwrap();
        let entry = RecordEntry::builder(7, tx.clone()).build();

        let wire = tool_policy_decision_from_entry(&entry).unwrap();
        assert_eq!(wire.verdict, "pending_approval");
        assert_eq!(wire.decided_by.source, "user");
        assert_eq!(wire.trace[0].source, "user_default");
        assert_eq!(wire.trace[0].outcome, "ask");
        assert_eq!(wire.approved_by.as_deref(), Some("user"));
        assert_eq!(wire.remember.as_deref(), Some("once"));
        assert_eq!(wire.seq, 7);
        assert_eq!(wire.tx_hash, tx.hash.to_hex());
        assert_eq!(wire.proposal_tx_hash, proposal_hash.to_hex());

        let json = serde_json::to_value(crate::OutboundMessage::ToolPolicyDecision(wire)).unwrap();
        assert_eq!(json["type"], "tool_policy_decision");

        let prompt = RecordEntry::builder(1, Transaction::user_prompt(agent_id, "hi")).build();
        assert!(tool_policy_decision_from_entry(&prompt).is_none());
    }
}
