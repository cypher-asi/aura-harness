//! Single-proposal tool processing flow.
//!
//! Used when the runtime hands the kernel exactly one [`ToolProposal`]
//! transaction (via `process_direct` / `process_dequeued`). The flow:
//!
//! 1. Append the bare `ToolProposal` entry (consuming the inbox token,
//!    when there is one).
//! 2. Run the policy + runtime-capability gate and resolve any live
//!    `ask` prompt, then append the `PolicyDecision` entry.
//! 3. **Phase 10 carve-out 5a**: fire `PreToolUse` BEFORE dispatch
//!    when a [`crate::kernel::KernelConfig::plugin_hooks`] is
//!    attached. On [`aura_plugin_hooks::HookOutcome::Block`] the
//!    executor is skipped and a synthetic
//!    [`aura_core_types::Effect::failed`] is surfaced so the agent loop
//!    sees a clean rejection.
//! 4. If approved (and not blocked), execute under the per-tool
//!    timeout, then append the `ToolExecution` entry.

use super::steps::finish_tool_call;
use crate::kernel::{Kernel, ProcessResult};
use aura_core_types::{ActionId, Effect, EffectKind, ToolProposal, Transaction};
use aura_exec_traits::ExecuteContext;
use aura_store_db::DequeueToken;
use bytes::Bytes;

pub(super) async fn process_one(
    kernel: &Kernel,
    tx: Transaction,
    token: Option<DequeueToken>,
) -> Result<ProcessResult, crate::KernelError> {
    let proposal: ToolProposal = serde_json::from_slice(&tx.payload)
        .map_err(|e| crate::KernelError::Serialization(format!("deserialize ToolProposal: {e}")))?;
    let runtime_capabilities = kernel.load_runtime_capabilities()?;

    let proposal_entry = kernel.record_tool_proposal(tx, token)?;
    let authorized = kernel
        .authorize_tool_proposal(proposal, &proposal_entry, runtime_capabilities.as_ref())
        .await?;

    let executed = if let Some(action) = authorized.action.clone() {
        let action_id = action.action_id;
        let proposal = &authorized.proposal;
        let tool_use_id = proposal.tool_use_id.clone();
        let tool_name = proposal.tool.clone();

        // Phase 10 carve-out 5a: fire `PreToolUse` BEFORE dispatch.
        // On `HookOutcome::Block` we never invoke the executor — a
        // synthetic failed [`Effect`] carrying the
        // `tool_call_blocked_by_hook` JSON-discriminated payload
        // takes the executor's place. The discriminator surfaces
        // the schema-v2 [`aura_store_record::RecordKind::ToolCallBlockedByHook`]
        // taxonomy at the effect-payload level so consumers can
        // distinguish a hook block from a normal executor failure.
        let blocked_effect = if let Some(host) = kernel.config.plugin_hooks.as_ref() {
            fire_pre_tool_use_and_maybe_block(host, proposal, action_id)?
        } else {
            None
        };

        let effect = if let Some(effect) = blocked_effect {
            effect
        } else {
            let workspace = kernel.agent_workspace();
            tokio::fs::create_dir_all(&workspace)
                .await
                .map_err(|e| crate::KernelError::Internal(format!("create workspace: {e}")))?;
            let ctx = ExecuteContext::new(kernel.agent_id, action_id, workspace)
                .with_tool_use_id(tool_use_id);
            kernel.execute_with_timeout(&ctx, &action, &tool_name).await
        };
        Some(kernel.record_tool_execution(&authorized, action, effect)?)
    } else {
        None
    };

    Ok(finish_tool_call(authorized, executed))
}

/// Fire `PreToolUse` and translate the aggregate outcome.
///
/// Returns `Ok(None)` for any non-blocking outcome (`Continue`,
/// `TimedOut`, `Approve`, `Deny`, `Replace`) — the dispatcher
/// proceeds normally in those cases.
///
/// Returns `Ok(Some(effect))` when a handler returned
/// [`aura_plugin_hooks::HookOutcome::Block`]. The synthetic
/// [`Effect`] carries a JSON-discriminated payload of the form
/// `{"kind": "tool_call_blocked_by_hook", "tool_name": ...,
/// "tool_use_id": ..., "reason": ...}` and
/// [`aura_core_types::EffectStatus::Failed`] as its status. The
/// discriminator surfaces the Phase 10 schema-v2
/// [`aura_store_record::RecordKind::ToolCallBlockedByHook`]
/// taxonomy at the effect-payload level so an auditor can
/// distinguish a hook block from a normal executor failure. The block
/// lands on the `ToolExecution` entry, after the `PolicyDecision` entry
/// that approved the call.
fn fire_pre_tool_use_and_maybe_block(
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
            "PreToolUse hook returned Block; dispatch aborted (Phase 10 5a)"
        );
        let effect = Effect::failed(action_id, EffectKind::Agreement, Bytes::from(bytes));
        return Ok(Some(effect));
    }
    Ok(None)
}
