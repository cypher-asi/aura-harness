//! Batch tool-proposal processing flow.
//!
//! Used when the runtime hands the kernel multiple [`ToolProposal`]s in
//! a single turn. Each proposal is recorded and then decided in order
//! (`ToolProposal`, `PolicyDecision`, ...), so every policy decision is
//! persisted before any tool runs. Approved proposals then execute
//! concurrently via `futures_util::join_all`, and their `ToolExecution`
//! entries are appended in proposal order.

use super::steps::{finish_tool_call, Authorized};
use crate::kernel::{Kernel, ProcessResult};
use aura_core_types::{Action, ActionId, Effect, EffectKind, ToolProposal, Transaction};
use aura_exec_traits::ExecuteContext;
use bytes::Bytes;

pub(super) async fn process_many(
    kernel: &Kernel,
    tool_proposals: Vec<ToolProposal>,
) -> Result<Vec<ProcessResult>, crate::KernelError> {
    if tool_proposals.is_empty() {
        return Ok(vec![]);
    }

    let runtime_capabilities = kernel.load_runtime_capabilities()?;
    let mut authorized: Vec<Authorized> = Vec::with_capacity(tool_proposals.len());
    for proposal in tool_proposals {
        let tx = Transaction::tool_proposal(kernel.agent_id, &proposal)
            .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;
        let proposal_entry = kernel.record_tool_proposal(tx, None)?;
        authorized.push(
            kernel
                .authorize_tool_proposal(proposal, &proposal_entry, runtime_capabilities.as_ref())
                .await?,
        );
    }

    let workspace = kernel.agent_workspace();
    tokio::fs::create_dir_all(&workspace)
        .await
        .map_err(|e| crate::KernelError::Internal(format!("create workspace: {e}")))?;

    // Phase 10 carve-out 5a: fire `PreToolUse` per-proposal BEFORE
    // executor dispatch. Any handler returning
    // `HookOutcome::Block` causes us to skip the executor for
    // that slot and substitute a synthetic failed
    // [`aura_core_types::Effect`] carrying the block reason, so the
    // `ToolExecution` entry distinguishes the block from a normal
    // executor failure.
    //
    // We pre-allocate per-proposal slots so the post-execution
    // pairing loop walks the same `(proposal, action, effect)`
    // tuples without a side index map.
    let total = authorized.len();
    let mut per_slot_effect: Vec<Option<Effect>> = vec![None; total];
    let mut dispatch_indices: Vec<usize> = Vec::new();
    let mut dispatch_contexts: Vec<ExecuteContext> = Vec::new();
    let mut dispatch_actions: Vec<Action> = Vec::new();
    let mut dispatch_tool_names: Vec<String> = Vec::new();

    for (i, slot) in authorized.iter().enumerate() {
        let Some(action) = slot.action.clone() else {
            continue;
        };
        let action_id = action.action_id;
        let proposal = &slot.proposal;

        if let Some(host) = kernel.config.plugin_hooks.as_ref() {
            if let Some(effect) = fire_pre_tool_use_for_batch(host, proposal, action_id)? {
                per_slot_effect[i] = Some(effect);
                continue;
            }
        }

        let ctx = ExecuteContext::new(kernel.agent_id, action_id, workspace.clone())
            .with_tool_use_id(proposal.tool_use_id.clone());
        dispatch_indices.push(i);
        dispatch_contexts.push(ctx);
        dispatch_actions.push(action);
        dispatch_tool_names.push(proposal.tool.clone());
    }

    let exec_futures = dispatch_contexts
        .iter()
        .zip(dispatch_actions.iter())
        .zip(dispatch_tool_names.iter())
        .map(|((ctx, action), tool_name)| kernel.execute_with_timeout(ctx, action, tool_name));
    let dispatched_effects: Vec<Effect> = futures_util::future::join_all(exec_futures).await;

    for (slot, effect) in dispatch_indices.iter().zip(dispatched_effects.into_iter()) {
        per_slot_effect[*slot] = Some(effect);
    }

    let mut results = Vec::with_capacity(total);
    for (slot, effect) in authorized.into_iter().zip(per_slot_effect) {
        let executed = match slot.action.clone() {
            Some(action) => {
                let effect = effect.expect("allowed verdict must have produced an effect");
                Some(kernel.record_tool_execution(&slot, action, effect)?)
            }
            None => None,
        };
        results.push(finish_tool_call(slot, executed));
    }

    Ok(results)
}

/// Phase 10 carve-out 5a — `PreToolUse` pre-dispatch firing for the
/// batch flow. Mirrors `single::fire_pre_tool_use_and_maybe_block`:
/// the synthetic [`Effect`] carries a `tool_call_blocked_by_hook`
/// JSON-discriminated payload and is recorded on the slot's
/// `ToolExecution` entry.
fn fire_pre_tool_use_for_batch(
    host: &aura_plugin_hooks::PluginHookHost,
    proposal: &ToolProposal,
    action_id: ActionId,
) -> Result<Option<Effect>, crate::KernelError> {
    if host.is_empty(aura_plugin_hooks::HookEvent::PreToolUse) {
        return Ok(None);
    }
    let args_text = serde_json::to_string(&proposal.args).unwrap_or_default();
    let outcome = host.fire_pre_tool_use(&proposal.tool, &args_text, &proposal.tool_use_id);
    if let aura_plugin_hooks::HookOutcome::Block { reason } = outcome.decision {
        let payload = serde_json::json!({
            "kind": "tool_call_blocked_by_hook",
            "tool_name": proposal.tool,
            "tool_use_id": proposal.tool_use_id,
            "reason": reason,
        });
        let bytes = serde_json::to_vec(&payload)
            .map_err(|e| crate::KernelError::Serialization(format!("block payload: {e}")))?;
        tracing::info!(
            tool_name = %proposal.tool,
            tool_use_id = %proposal.tool_use_id,
            "PreToolUse hook returned Block; dispatch aborted (Phase 10 5a, batch path)"
        );
        let effect = Effect::failed(action_id, EffectKind::Agreement, Bytes::from(bytes));
        return Ok(Some(effect));
    }
    Ok(None)
}
