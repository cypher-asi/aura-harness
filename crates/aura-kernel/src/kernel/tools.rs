//! Tool-proposal processing.
//!
//! Single-proposal (`process_tool_proposal`) and batch (`process_tools`)
//! paths both:
//! 1. Route each proposal through the full
//!    `Policy::check_with_runtime_capabilities_verdict` pipeline
//!    (Invariant §4).
//! 2. If the verdict is [`crate::PolicyVerdict::RequireApproval`],
//!    consult the shared [`crate::ApprovalRegistry`]: a matching
//!    single-use approval unlocks the call and is consumed; otherwise
//!    the call is denied with a structured
//!    [`crate::ToolDecision::NeedsApproval`] surfaced on the
//!    [`ProcessResult`].
//! 3. Execute approved tools via `ExecutorRouter`.
//! 4. Build a `RecordEntry` with the proposal set, decision, actions,
//!    and effects attached (Invariant §5).
//!
//! The batch path additionally reserves a contiguous sequence range and
//! writes all entries via `append_entries_batch` for atomicity.

use super::{ApprovalRequiredInfo, Kernel, ProcessResult, ToolDecision, ToolOutput};
use crate::context::hash_tx_with_window;
use crate::executor::{decode_tool_effect, ExecuteContext};
use crate::policy::{ApprovalKey, PolicyVerdict};
use aura_core::{
    Action, ActionId, ActionKind, ContextHash, Decision, Effect, EffectKind, EffectStatus,
    Proposal, ProposalSet, RecordEntry, ToolCall, ToolProposal, Transaction,
};
use std::time::Duration;
use tracing::instrument;

impl Kernel {
    // -----------------------------------------------------------------------
    // Tool proposal processing
    // -----------------------------------------------------------------------

    #[instrument(skip(self, tx), fields(seq))]
    pub(super) async fn process_tool_proposal(
        &self,
        tx: &Transaction,
        seq: u64,
        context_hash: ContextHash,
    ) -> Result<ProcessResult, crate::KernelError> {
        let proposal: ToolProposal = serde_json::from_slice(&tx.payload).map_err(|e| {
            crate::KernelError::Serialization(format!("deserialize ToolProposal: {e}"))
        })?;

        let tool_use_id = proposal.tool_use_id.clone();
        let tool_name = proposal.tool.clone();
        let args_hash = ApprovalKey::hash_args(&proposal.args);

        let kernel_proposal = Proposal::new(
            ActionKind::Delegate,
            serde_json::to_vec(&ToolCall::new(&proposal.tool, proposal.args.clone()))
                .map_err(|e| crate::KernelError::Serialization(e.to_string()))?,
        );

        let runtime_capabilities = self.load_runtime_capabilities()?;
        let verdict = self.policy.check_with_runtime_capabilities_verdict(
            &kernel_proposal,
            runtime_capabilities.as_ref(),
        );

        let approval_unlocked = matches!(verdict, PolicyVerdict::RequireApproval { .. })
            && self.approvals.take(self.agent_id, &tool_name, args_hash);
        let effective_allowed = verdict.is_allowed() || approval_unlocked;

        if effective_allowed {
            let action_id = ActionId::generate();
            let action = Action::new(
                action_id,
                ActionKind::Delegate,
                kernel_proposal.payload.clone(),
            );

            let workspace = self.agent_workspace();
            tokio::fs::create_dir_all(&workspace)
                .await
                .map_err(|e| crate::KernelError::Internal(format!("create workspace: {e}")))?;
            let ctx = ExecuteContext::new(self.agent_id, action_id, workspace);
            let effect = self.executor.execute(&ctx, &action).await;

            let had_failures = effect.status == EffectStatus::Failed;
            let output_content = decode_tool_effect(&effect).content;

            let mut decision = Decision::new();
            decision.accept(action_id);

            let mut proposals = ProposalSet::new();
            proposals.proposals.push(kernel_proposal);

            let entry = RecordEntry::builder(seq, tx.clone())
                .context_hash(context_hash)
                .proposals(proposals)
                .decision(decision)
                .actions(vec![action])
                .effects(vec![effect])
                .build();

            Ok(ProcessResult {
                entry,
                tool_output: Some(ToolOutput {
                    tool_use_id,
                    content: output_content,
                    is_error: had_failures,
                    approval_required: None,
                }),
                had_failures,
                runtime_capability_update: None,
                clear_runtime_capabilities: false,
                tool_decision: Some(ToolDecision::Allowed),
            })
        } else {
            let denial_reason = verdict
                .reason()
                .map_or_else(|| "Policy denied".to_string(), str::to_string);
            let needs_approval = matches!(verdict, PolicyVerdict::RequireApproval { .. });

            let mut decision = Decision::new();
            #[allow(clippy::cast_possible_truncation)]
            decision.reject(0, &denial_reason);

            let mut proposals = ProposalSet::new();
            proposals.proposals.push(kernel_proposal);

            let entry = RecordEntry::builder(seq, tx.clone())
                .context_hash(context_hash)
                .proposals(proposals)
                .decision(decision)
                .build();

            let (approval_required, tool_decision) = if needs_approval {
                (
                    Some(ApprovalRequiredInfo {
                        tool: tool_name.clone(),
                        args_hash,
                    }),
                    ToolDecision::NeedsApproval {
                        reason: denial_reason.clone(),
                        args_hash,
                    },
                )
            } else {
                (
                    None,
                    ToolDecision::Denied {
                        reason: denial_reason.clone(),
                    },
                )
            };

            Ok(ProcessResult {
                entry,
                tool_output: Some(ToolOutput {
                    tool_use_id,
                    content: denial_reason,
                    is_error: true,
                    approval_required,
                }),
                had_failures: false,
                runtime_capability_update: None,
                clear_runtime_capabilities: false,
                tool_decision: Some(tool_decision),
            })
        }
    }

    // -----------------------------------------------------------------------
    // Batch tool processing
    // -----------------------------------------------------------------------

    /// Process a batch of tool proposals, executing approved tools in parallel.
    ///
    /// # Errors
    /// Returns error if serialization, execution, or storage fails.
    #[allow(clippy::too_many_lines)]
    pub async fn process_tools(
        &self,
        tool_proposals: Vec<ToolProposal>,
    ) -> Result<Vec<ProcessResult>, crate::KernelError> {
        if tool_proposals.is_empty() {
            return Ok(vec![]);
        }

        let mut kernel_proposals: Vec<Proposal> = Vec::with_capacity(tool_proposals.len());
        let mut verdicts: Vec<(PolicyVerdict, bool)> = Vec::with_capacity(tool_proposals.len());
        let mut args_hashes: Vec<[u8; 32]> = Vec::with_capacity(tool_proposals.len());
        let runtime_capabilities = self.load_runtime_capabilities()?;

        for proposal in &tool_proposals {
            let tool_call = ToolCall::new(&proposal.tool, proposal.args.clone());
            let payload = serde_json::to_vec(&tool_call)
                .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;
            let kernel_proposal = Proposal::new(ActionKind::Delegate, payload);
            let verdict = self.policy.check_with_runtime_capabilities_verdict(
                &kernel_proposal,
                runtime_capabilities.as_ref(),
            );
            let args_hash = ApprovalKey::hash_args(&proposal.args);
            let approval_unlocked = matches!(verdict, PolicyVerdict::RequireApproval { .. })
                && self
                    .approvals
                    .take(self.agent_id, &proposal.tool, args_hash);
            verdicts.push((verdict, approval_unlocked));
            args_hashes.push(args_hash);
            kernel_proposals.push(kernel_proposal);
        }

        let workspace = self.agent_workspace();
        tokio::fs::create_dir_all(&workspace)
            .await
            .map_err(|e| crate::KernelError::Internal(format!("create workspace: {e}")))?;

        let mut exec_contexts: Vec<ExecuteContext> = Vec::new();
        let mut exec_actions: Vec<Action> = Vec::new();

        for (i, proposal) in tool_proposals.iter().enumerate() {
            let (verdict, approval_unlocked) = &verdicts[i];
            if !(verdict.is_allowed() || *approval_unlocked) {
                continue;
            }
            let action_id = ActionId::generate();
            let tool_call = ToolCall::new(&proposal.tool, proposal.args.clone());
            let payload = serde_json::to_vec(&tool_call)
                .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;
            let action = Action::new(action_id, ActionKind::Delegate, payload);
            let ctx = ExecuteContext::new(self.agent_id, action_id, workspace.clone());

            exec_contexts.push(ctx);
            exec_actions.push(action);
        }

        let tool_timeout = Duration::from_millis(self.config.tool_timeout_ms);
        let exec_futures =
            exec_contexts
                .iter()
                .zip(exec_actions.iter())
                .map(|(ctx, action)| async move {
                    match tokio::time::timeout(tool_timeout, self.executor.execute(ctx, action))
                        .await
                    {
                        Ok(effect) => effect,
                        Err(_) => {
                            tracing::warn!(
                                action_id = %action.action_id,
                                timeout_ms = self.config.tool_timeout_ms,
                                "Tool execution timed out"
                            );
                            Effect::failed(
                                action.action_id,
                                EffectKind::Agreement,
                                format!("Tool timed out after {}ms", self.config.tool_timeout_ms),
                            )
                        }
                    }
                });

        let effects: Vec<Effect> = futures_util::future::join_all(exec_futures).await;

        let total = tool_proposals.len();
        let base_seq = self.reserve_seq_range(total);

        let mut results = Vec::with_capacity(total);
        let mut entries = Vec::with_capacity(total);
        let mut approved_idx = 0;

        for (i, proposal) in tool_proposals.iter().enumerate() {
            let seq = base_seq + i as u64;
            let tx = Transaction::tool_proposal(self.agent_id, proposal)
                .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;

            let window = self.load_window(seq)?;
            let context_hash = hash_tx_with_window(&tx, &window)?;
            let tool_use_id = proposal.tool_use_id.clone();

            let (verdict, approval_unlocked) = &verdicts[i];
            let was_approved = verdict.is_allowed() || *approval_unlocked;

            if was_approved {
                let action = &exec_actions[approved_idx];
                let effect = &effects[approved_idx];
                approved_idx += 1;

                let had_failures = effect.status == EffectStatus::Failed;
                let output_content = decode_tool_effect(effect).content;

                let mut decision = Decision::new();
                decision.accept(action.action_id);

                let mut proposal_set = ProposalSet::new();
                proposal_set.proposals.push(kernel_proposals[i].clone());

                let entry = RecordEntry::builder(seq, tx)
                    .context_hash(context_hash)
                    .proposals(proposal_set)
                    .decision(decision)
                    .actions(vec![action.clone()])
                    .effects(vec![effect.clone()])
                    .build();

                entries.push(entry.clone());
                results.push(ProcessResult {
                    entry,
                    tool_output: Some(ToolOutput {
                        tool_use_id,
                        content: output_content,
                        is_error: had_failures,
                        approval_required: None,
                    }),
                    had_failures,
                    runtime_capability_update: None,
                    clear_runtime_capabilities: false,
                    tool_decision: Some(ToolDecision::Allowed),
                });
            } else {
                let denial_reason = verdict
                    .reason()
                    .map_or_else(|| "Policy denied".to_string(), str::to_string);
                let needs_approval = matches!(verdict, PolicyVerdict::RequireApproval { .. });
                let args_hash = args_hashes[i];
                let mut decision = Decision::new();
                decision.reject(0, &denial_reason);

                let mut proposal_set = ProposalSet::new();
                proposal_set.proposals.push(kernel_proposals[i].clone());

                let entry = RecordEntry::builder(seq, tx)
                    .context_hash(context_hash)
                    .proposals(proposal_set)
                    .decision(decision)
                    .build();

                let (approval_required, tool_decision) = if needs_approval {
                    (
                        Some(ApprovalRequiredInfo {
                            tool: proposal.tool.clone(),
                            args_hash,
                        }),
                        ToolDecision::NeedsApproval {
                            reason: denial_reason.clone(),
                            args_hash,
                        },
                    )
                } else {
                    (
                        None,
                        ToolDecision::Denied {
                            reason: denial_reason.clone(),
                        },
                    )
                };

                entries.push(entry.clone());
                results.push(ProcessResult {
                    entry,
                    tool_output: Some(ToolOutput {
                        tool_use_id,
                        content: denial_reason,
                        is_error: true,
                        approval_required,
                    }),
                    had_failures: false,
                    runtime_capability_update: None,
                    clear_runtime_capabilities: false,
                    tool_decision: Some(tool_decision),
                });
            }
        }

        self.store
            .append_entries_batch(self.agent_id, base_seq, &entries)
            .map_err(|e| crate::KernelError::Store(format!("append_entries_batch: {e}")))?;

        Ok(results)
    }
}
