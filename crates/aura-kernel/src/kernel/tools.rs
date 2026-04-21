//! Tool-proposal processing.
//!
//! Single-proposal (`process_tool_proposal`) and batch (`process_tools`)
//! paths both:
//! 1. Route each proposal through the full `Policy::check_with_runtime_capabilities`
//!    pipeline (Invariant §4).
//! 2. Execute approved tools via `ExecutorRouter`.
//! 3. Build a `RecordEntry` with the proposal set, decision, actions, and
//!    effects attached (Invariant §5).
//!
//! The batch path additionally reserves a contiguous sequence range and
//! writes all entries via `append_entries_batch` for atomicity.

use super::{Kernel, ProcessResult, ToolOutput};
use crate::context::hash_tx_with_window;
use crate::executor::{decode_tool_effect, ExecuteContext};
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

        let kernel_proposal = Proposal::new(
            ActionKind::Delegate,
            serde_json::to_vec(&ToolCall::new(&proposal.tool, proposal.args.clone()))
                .map_err(|e| crate::KernelError::Serialization(e.to_string()))?,
        );

        let runtime_capabilities = self.load_runtime_capabilities()?;
        // Route through the full policy pipeline (action_kind allow-list
        // → tool allow-list → permission level → agent permissions) per
        // Invariant §4. The narrower `check_tool_with_runtime_capabilities`
        // by itself bypasses `allowed_action_kinds` and
        // `check_agent_permissions`.
        let policy_result = self
            .policy
            .check_with_runtime_capabilities(&kernel_proposal, runtime_capabilities.as_ref());

        if policy_result.allowed {
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
            // Decode the ToolResult so the LLM sees the tool's actual
            // stdout/stderr as plain UTF-8 text. Using the raw JSON
            // payload here surfaces base64-encoded byte fields (see
            // `aura_core::serde_helpers::bytes_serde`) directly to the
            // model, which then parrots them back into chat as
            // `binary` noise.
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
                }),
                had_failures,
                runtime_capability_update: None,
                clear_runtime_capabilities: false,
            })
        } else {
            let mut decision = Decision::new();
            #[allow(clippy::cast_possible_truncation)]
            decision.reject(0, policy_result.reason.as_deref().unwrap_or("denied"));

            let mut proposals = ProposalSet::new();
            proposals.proposals.push(kernel_proposal);

            let denial_reason = policy_result
                .reason
                .unwrap_or_else(|| "Policy denied".to_string());

            let entry = RecordEntry::builder(seq, tx.clone())
                .context_hash(context_hash)
                .proposals(proposals)
                .decision(decision)
                .build();

            Ok(ProcessResult {
                entry,
                tool_output: Some(ToolOutput {
                    tool_use_id,
                    content: denial_reason,
                    is_error: true,
                }),
                had_failures: false,
                runtime_capability_update: None,
                clear_runtime_capabilities: false,
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

        // Classify each proposal via the full policy pipeline
        // (Invariant §4). Build the kernel-level `Proposal` once per
        // tool proposal so the same value drives both the authorization
        // decision and the later `RecordEntry` that captures it.
        let mut approved = Vec::new();
        let mut denied = Vec::new();
        let mut kernel_proposals: Vec<Proposal> = Vec::with_capacity(tool_proposals.len());
        let runtime_capabilities = self.load_runtime_capabilities()?;

        for proposal in &tool_proposals {
            let tool_call = ToolCall::new(&proposal.tool, proposal.args.clone());
            let payload = serde_json::to_vec(&tool_call)
                .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;
            let kernel_proposal = Proposal::new(ActionKind::Delegate, payload);
            let result = self
                .policy
                .check_with_runtime_capabilities(&kernel_proposal, runtime_capabilities.as_ref());
            if result.allowed {
                approved.push(proposal);
            } else {
                denied.push((proposal, result));
            }
            kernel_proposals.push(kernel_proposal);
        }

        // Prepare workspace
        let workspace = self.agent_workspace();
        tokio::fs::create_dir_all(&workspace)
            .await
            .map_err(|e| crate::KernelError::Internal(format!("create workspace: {e}")))?;

        // Build actions and contexts for all approved tools
        let mut exec_contexts: Vec<ExecuteContext> = Vec::new();
        let mut exec_actions: Vec<(&ToolProposal, Action)> = Vec::new();

        for proposal in &approved {
            let action_id = ActionId::generate();
            let tool_call = ToolCall::new(&proposal.tool, proposal.args.clone());
            let payload = serde_json::to_vec(&tool_call)
                .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;
            let action = Action::new(action_id, ActionKind::Delegate, payload);
            let ctx = ExecuteContext::new(self.agent_id, action_id, workspace.clone());

            exec_contexts.push(ctx);
            exec_actions.push((proposal, action));
        }

        // Execute in parallel — borrow from the collected Vecs so lifetimes work.
        // Each tool is wrapped in a per-tool timeout driven by `tool_timeout_ms`
        // so a slow tool cannot block the whole batch.
        let tool_timeout = Duration::from_millis(self.config.tool_timeout_ms);
        let exec_futures =
            exec_contexts
                .iter()
                .zip(exec_actions.iter())
                .map(|(ctx, (_, action))| async move {
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

        // Reserve contiguous sequences for ALL results
        let total = tool_proposals.len();
        let base_seq = self.reserve_seq_range(total);

        // Build results in input order
        let mut results = Vec::with_capacity(total);
        let mut entries = Vec::with_capacity(total);
        let mut approved_idx = 0;
        let mut denied_idx = 0;

        for (i, proposal) in tool_proposals.iter().enumerate() {
            let seq = base_seq + i as u64;
            let tx = Transaction::tool_proposal(self.agent_id, proposal)
                .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;

            let window = self.load_window(seq)?;
            let context_hash = hash_tx_with_window(&tx, &window)?;
            let tool_use_id = proposal.tool_use_id.clone();

            // Reuse the authorization result from the classification step
            // above — never re-run `check_*` here with a narrower check.
            let was_approved = self
                .policy
                .check_with_runtime_capabilities(
                    &kernel_proposals[i],
                    runtime_capabilities.as_ref(),
                )
                .allowed;

            if was_approved {
                let (_, action) = &exec_actions[approved_idx];
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
                    }),
                    had_failures,
                    runtime_capability_update: None,
                    clear_runtime_capabilities: false,
                });
            } else {
                let (_, policy_result) = &denied[denied_idx];
                denied_idx += 1;

                let denial_reason = policy_result
                    .reason
                    .clone()
                    .unwrap_or_else(|| "Policy denied".to_string());
                let mut decision = Decision::new();
                decision.reject(0, &denial_reason);

                let mut proposal_set = ProposalSet::new();
                proposal_set.proposals.push(kernel_proposals[i].clone());

                let entry = RecordEntry::builder(seq, tx)
                    .context_hash(context_hash)
                    .proposals(proposal_set)
                    .decision(decision)
                    .build();

                entries.push(entry.clone());
                results.push(ProcessResult {
                    entry,
                    tool_output: Some(ToolOutput {
                        tool_use_id,
                        content: denial_reason,
                        is_error: true,
                    }),
                    had_failures: false,
                    runtime_capability_update: None,
                    clear_runtime_capabilities: false,
                });
            }
        }

        // Atomic batch write
        self.store
            .append_entries_batch(self.agent_id, base_seq, &entries)
            .map_err(|e| crate::KernelError::Store(format!("append_entries_batch: {e}")))?;

        Ok(results)
    }
}
