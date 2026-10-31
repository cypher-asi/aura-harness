//! The three recorded steps of a tool call.
//!
//! 1. `ToolProposal` — the bare proposal, appended before policy runs.
//! 2. `PolicyDecision` — the verdict and the rule that produced it,
//!    referencing the proposal. Appended before the executor runs.
//! 3. `ToolExecution` — the action + effect, referencing the decision.
//!    Only written for approved calls.
//!
//! Each entry is appended on its own sequence number as soon as it
//! exists, so its context-hash window always matches what replay
//! recomputes from the store.

use super::shared::maybe_summarise_effect_payload;
use crate::context::hash_tx_with_window;
use crate::kernel::{
    ApprovalRequiredInfo, Kernel, PolicyDecisionRef, ProcessResult, ToolDecision, ToolOutput,
};
use crate::policy::{PolicyEvaluation, PolicyVerdict};
use aura_core_types::{
    Action, ActionId, ActionKind, Decision, Effect, EffectStatus, PolicyDecision, ProposalSet,
    RecordEntry, RecordEntryBuilder, RuntimeCapabilityInstall, ToolExecution, ToolGateVerdict,
    ToolProposal, Transaction,
};
use aura_exec_traits::{decode_tool_effect, DecodedToolResult};
use aura_store_db::DequeueToken;

/// A tool proposal whose policy decision has been recorded.
pub(super) struct Authorized {
    pub proposal: ToolProposal,
    pub evaluation: PolicyEvaluation,
    /// `Some` iff the verdict allowed the call.
    pub action: Option<Action>,
    pub decision_entry: RecordEntry,
    pub decision_ref: PolicyDecisionRef,
}

/// A recorded `ToolExecution` entry plus the decoded tool output.
pub(super) struct Executed {
    pub entry: RecordEntry,
    pub decoded: DecodedToolResult,
    pub had_failures: bool,
}

impl Kernel {
    fn append_tool_entry(
        &self,
        tx: Transaction,
        fill: impl FnOnce(RecordEntryBuilder) -> RecordEntryBuilder,
        token: Option<DequeueToken>,
    ) -> Result<RecordEntry, crate::KernelError> {
        let seq = self.next_seq()?;
        let window = self.load_window(seq)?;
        let context_hash = hash_tx_with_window(&tx, &window)?;
        let entry = fill(RecordEntry::builder(seq, tx).context_hash(context_hash)).build();
        match token {
            Some(token) => self
                .store
                .append_entry_dequeued(self.agent_id, seq, &entry, token)
                .map_err(|e| crate::KernelError::Store(format!("append_entry_dequeued: {e}")))?,
            None => self
                .store
                .append_entry_direct(self.agent_id, seq, &entry)
                .map_err(|e| crate::KernelError::Store(format!("append_entry_direct: {e}")))?,
        }
        if let Some(observer) = self.config.tool_audit_observer.as_ref() {
            observer.on_tool_entry(&entry);
        }
        Ok(entry)
    }

    /// Step 1: append the bare `ToolProposal` entry.
    pub(super) fn record_tool_proposal(
        &self,
        tx: Transaction,
        token: Option<DequeueToken>,
    ) -> Result<RecordEntry, crate::KernelError> {
        self.append_tool_entry(tx, |builder| builder, token)
    }

    /// Step 2: run the policy (including any live approval) and append
    /// the `PolicyDecision` entry.
    pub(super) async fn authorize_tool_proposal(
        &self,
        proposal: ToolProposal,
        proposal_entry: &RecordEntry,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
    ) -> Result<Authorized, crate::KernelError> {
        let kernel_proposal = self.kernel_proposal_from_tool_proposal(&proposal)?;
        let evaluation = self
            .policy
            .evaluate_proposal(&kernel_proposal, runtime_capabilities);
        let evaluation = self
            .resolve_live_ask(
                &proposal.tool,
                &proposal.args,
                &proposal.tool_use_id,
                evaluation,
            )
            .await?;

        let action = evaluation.verdict.is_allowed().then(|| {
            Action::new(
                ActionId::generate(),
                ActionKind::Delegate,
                kernel_proposal.payload.clone(),
            )
        });

        let decision = PolicyDecision {
            tool_use_id: proposal.tool_use_id.clone(),
            tool: proposal.tool.clone(),
            verdict: gate_verdict(&evaluation.verdict),
            reason: evaluation.verdict.reason().map(str::to_string),
            decided_by: evaluation.decided_by.clone(),
            trace: evaluation.trace.clone(),
            approval: evaluation.approval.clone(),
            mode: self.policy.mode_label(),
            policy_hash: self.policy.snapshot_hash().to_string(),
            action_id: action.as_ref().map(|a| a.action_id.to_hex()),
        };
        let tx = Transaction::policy_decision(self.agent_id, &decision, proposal_entry.tx.hash)
            .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;

        let mut proposals = ProposalSet::new();
        proposals.proposals.push(kernel_proposal);
        let mut kernel_decision = Decision::new();
        match &action {
            Some(action) => kernel_decision.accept(action.action_id),
            None => kernel_decision.reject(0, denial_reason(&evaluation.verdict)),
        }
        let decision_entry = self.append_tool_entry(
            tx,
            |builder| builder.proposals(proposals).decision(kernel_decision),
            None,
        )?;

        let decision_ref = PolicyDecisionRef {
            proposal_seq: proposal_entry.seq,
            proposal_tx_hash: proposal_entry.tx.hash,
            seq: decision_entry.seq,
            tx_hash: decision_entry.tx.hash,
            decision,
        };
        Ok(Authorized {
            proposal,
            evaluation,
            action,
            decision_entry,
            decision_ref,
        })
    }

    /// Step 3: append the `ToolExecution` entry for an approved call.
    pub(super) fn record_tool_execution(
        &self,
        authorized: &Authorized,
        action: Action,
        effect: Effect,
    ) -> Result<Executed, crate::KernelError> {
        let effect = maybe_summarise_effect_payload(effect, self.lite_payload_threshold());
        let decoded = decode_tool_effect(&effect);
        let had_failures = effect.status == EffectStatus::Failed || decoded.is_error;

        let execution = ToolExecution {
            tool_use_id: authorized.proposal.tool_use_id.clone(),
            tool: authorized.proposal.tool.clone(),
            args: authorized.proposal.args.clone(),
            decision: ToolGateVerdict::Approved,
            reason: None,
            result: None,
            is_error: had_failures,
            parent_agent_id: self.agent_id,
            originating_user_id: self.config.originating_user_id.clone().unwrap_or_default(),
        };
        let tx = Transaction::tool_execution_for_decision(
            self.agent_id,
            &execution,
            authorized.decision_entry.tx.hash,
        )
        .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;
        let entry = self.append_tool_entry(
            tx,
            |builder| builder.actions(vec![action]).effects(vec![effect]),
            None,
        )?;
        Ok(Executed {
            entry,
            decoded,
            had_failures,
        })
    }
}

/// Build the caller-facing [`ProcessResult`] for a tool call.
///
/// Contract: `executed.is_some()` iff `authorized.action.is_some()`.
pub(super) fn finish_tool_call(
    authorized: Authorized,
    executed: Option<Executed>,
) -> ProcessResult {
    let tool_use_id = authorized.proposal.tool_use_id.clone();
    if let Some(Executed {
        entry,
        decoded,
        had_failures,
    }) = executed
    {
        // Drop any image on a failed effect: the screenshot is only
        // meaningful for a successful computer-use action.
        let image = if had_failures { None } else { decoded.image };
        return ProcessResult {
            entry,
            tool_output: Some(ToolOutput {
                tool_use_id,
                content: decoded.content,
                is_error: had_failures,
                kind: decoded.kind,
                approval_required: None,
                line_diff: decoded.line_diff,
                image,
            }),
            had_failures,
            runtime_capability_update: None,
            clear_runtime_capabilities: false,
            tool_decision: Some(ToolDecision::Allowed),
            policy_decision: Some(authorized.decision_ref),
        };
    }

    let verdict = &authorized.evaluation.verdict;
    let reason = denial_reason(verdict);
    let (approval_required, tool_decision) = match verdict {
        PolicyVerdict::RequireApproval { prompt, .. } => (
            Some(ApprovalRequiredInfo {
                tool: authorized.proposal.tool.clone(),
                prompt: prompt.clone(),
            }),
            ToolDecision::NeedsApproval {
                reason: reason.clone(),
                prompt: prompt.clone(),
            },
        ),
        _ => (
            None,
            ToolDecision::Denied {
                reason: reason.clone(),
            },
        ),
    };

    ProcessResult {
        entry: authorized.decision_entry,
        tool_output: Some(ToolOutput {
            tool_use_id,
            content: reason,
            is_error: true,
            kind: aura_core_types::ToolResultKind::AgentError,
            approval_required,
            line_diff: None,
            image: None,
        }),
        had_failures: false,
        runtime_capability_update: None,
        clear_runtime_capabilities: false,
        tool_decision: Some(tool_decision),
        policy_decision: Some(authorized.decision_ref),
    }
}

fn denial_reason(verdict: &PolicyVerdict) -> String {
    verdict
        .reason()
        .map_or_else(|| "Policy denied".to_string(), str::to_string)
}

const fn gate_verdict(verdict: &PolicyVerdict) -> ToolGateVerdict {
    match verdict {
        PolicyVerdict::Allow => ToolGateVerdict::Approved,
        PolicyVerdict::RequireApproval { .. } => ToolGateVerdict::PendingApproval,
        PolicyVerdict::Deny { .. } => ToolGateVerdict::Denied,
    }
}
