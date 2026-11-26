//! Policy decisions as chat lines and record details.

use super::*;

/// One-line chat rendering of a policy decision, e.g.
/// `policy: read_file allowed by user_default.full_access`.
///
/// `None` when the user's own command was allowed: its output already
/// shows that it ran.
#[must_use]
pub fn policy_chat_line(decision: &PolicyDecision) -> Option<String> {
    policy_outcome(decision).map(|outcome| format!("policy: {} {outcome}", decision.tool))
}

/// The verdict and deciding rule of a policy decision, e.g.
/// `allowed by user_default.full_access`; `None` as for
/// [`policy_chat_line`].
#[must_use]
pub fn policy_outcome(decision: &PolicyDecision) -> Option<String> {
    let verdict = match decision.verdict {
        ToolGateVerdict::Approved if is_user_tool_use(&decision.tool_use_id) => return None,
        ToolGateVerdict::Approved => "allowed",
        ToolGateVerdict::Denied => "denied",
        ToolGateVerdict::PendingApproval => "needs approval",
    };
    Some(format!("{verdict} by {}", decision.decided_by))
}

/// Policy provenance for the record detail view; `None` for entries that
/// are not policy decisions.
pub(super) fn policy_detail(tx: &Transaction) -> Option<PolicyDetail> {
    if tx.tx_type != TransactionType::PolicyDecision {
        return None;
    }
    let decision = serde_json::from_slice::<PolicyDecision>(&tx.payload).ok()?;
    let approval = decision
        .approval
        .as_ref()
        .map_or_else(String::new, |approval| {
            let approver = match &approval.approver {
                PolicyApprover::Hook => "hook".to_string(),
                PolicyApprover::User { user_id: Some(id) } => format!("user {id}"),
                PolicyApprover::User { user_id: None } => "user".to_string(),
            };
            match approval.remember.as_deref() {
                Some(remember) => format!("{approver} ({remember})"),
                None => approver,
            }
        });
    Some(PolicyDetail {
        tool: decision.tool.clone(),
        verdict: verdict_label(decision.verdict).to_string(),
        decided_by: decision.decided_by.to_string(),
        reason: decision.reason.clone().unwrap_or_default(),
        trace: decision
            .trace
            .iter()
            .map(|check| {
                let outcome = match check.outcome {
                    PolicyCheckOutcome::Allow => "allow",
                    PolicyCheckOutcome::Deny => "deny ",
                    PolicyCheckOutcome::Ask => "ask  ",
                };
                format!("{outcome} {}", check.rule)
            })
            .collect(),
        approval,
        mode: decision.mode,
        policy_hash: decision.policy_hash,
        proposal_tx: tx
            .reference_tx_hash
            .map(|hash| hash.to_hex())
            .unwrap_or_default(),
    })
}

pub(super) const fn verdict_label(verdict: ToolGateVerdict) -> &'static str {
    match verdict {
        ToolGateVerdict::Approved => "approved",
        ToolGateVerdict::Denied => "denied",
        ToolGateVerdict::PendingApproval => "pending_approval",
    }
}
