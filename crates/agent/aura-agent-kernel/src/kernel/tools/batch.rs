//! Batch tool-proposal processing flow.
//!
//! Used when the runtime hands the kernel multiple [`ToolProposal`]s in
//! a single turn. Each proposal is recorded and then decided in order
//! (`ToolProposal`, `PolicyDecision`, ...), so every policy decision is
//! persisted before any tool runs. Approved proposals then execute
//! concurrently via `futures_util::join_all`, and their `ToolExecution`
//! entries are appended in proposal order.
//!
//! A call may bring its own result ([`ToolRun::Preset`]) or handler
//! ([`ToolRun::Local`]). It is recorded and gated like any other call;
//! when policy allows it the preset or handler stands in for the executor,
//! and when policy denies it the denial wins and the handler never runs.

use super::steps::{finish_tool_call, Authorized};
use super::{PresetToolResult, ToolRun};
use crate::kernel::stream::elapsed_ms;
use crate::kernel::{Kernel, ProcessResult};
use aura_core_types::{ActionId, Effect, EffectKind, ToolProposal, ToolResult, Transaction};
use aura_exec_traits::ExecuteContext;
use bytes::Bytes;
use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

type SlotFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(Effect, u64), crate::KernelError>> + Send + 'a>>;

pub(super) async fn process_many<'a>(
    kernel: &'a Kernel,
    calls: Vec<(ToolProposal, ToolRun<'a>)>,
) -> Result<Vec<ProcessResult>, crate::KernelError> {
    if calls.is_empty() {
        return Ok(vec![]);
    }

    let (authorized, runs) = authorize_all(kernel, calls).await?;

    let workspace = kernel.agent_workspace();
    tokio::fs::create_dir_all(&workspace)
        .await
        .map_err(|e| crate::KernelError::Internal(format!("create workspace: {e}")))?;

    let per_slot_effect = dispatch_all(kernel, &authorized, runs, &workspace).await?;
    record_all(kernel, authorized, per_slot_effect)
}

/// Record and decide every proposal in order, so each policy decision is
/// persisted before any tool runs.
async fn authorize_all<'a>(
    kernel: &'a Kernel,
    calls: Vec<(ToolProposal, ToolRun<'a>)>,
) -> Result<(Vec<Authorized>, Vec<ToolRun<'a>>), crate::KernelError> {
    let runtime_capabilities = kernel.load_runtime_capabilities()?;
    let mut authorized: Vec<Authorized> = Vec::with_capacity(calls.len());
    let mut runs: Vec<ToolRun<'a>> = Vec::with_capacity(calls.len());
    for (proposal, run) in calls {
        let tx = Transaction::tool_proposal(kernel.agent_id, &proposal)
            .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;
        let proposal_entry = kernel.record_tool_proposal(tx, None)?;
        authorized.push(
            kernel
                .authorize_tool_proposal(proposal, &proposal_entry, runtime_capabilities.as_ref())
                .await?,
        );
        runs.push(run);
    }
    Ok((authorized, runs))
}

/// Produce one effect per allowed slot: a hook block, a preset, or the
/// result of a concurrently dispatched future.
async fn dispatch_all<'a>(
    kernel: &'a Kernel,
    authorized: &[Authorized],
    runs: Vec<ToolRun<'a>>,
    workspace: &std::path::Path,
) -> Result<Vec<Option<(Effect, u64)>>, crate::KernelError> {
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
    let mut per_slot_effect: Vec<Option<(Effect, u64)>> = vec![None; total];
    let mut dispatch_indices: Vec<usize> = Vec::new();
    let mut dispatch_futures: Vec<SlotFuture<'a>> = Vec::new();

    for (i, (slot, run)) in authorized.iter().zip(runs).enumerate() {
        match slot_future(kernel, slot, run, workspace)? {
            SlotDispatch::Skip => {}
            SlotDispatch::Ready(effect) => per_slot_effect[i] = Some(effect),
            SlotDispatch::Pending(future) => {
                dispatch_indices.push(i);
                dispatch_futures.push(future);
            }
        }
    }

    let dispatched_effects = futures_util::future::join_all(dispatch_futures).await;

    for (slot, effect) in dispatch_indices.iter().zip(dispatched_effects.into_iter()) {
        per_slot_effect[*slot] = Some(effect?);
    }
    Ok(per_slot_effect)
}

/// What one slot contributes to the dispatch phase.
enum SlotDispatch<'a> {
    /// Denied by policy: no action, no effect.
    Skip,
    /// Effect known without running anything (hook block or preset).
    Ready((Effect, u64)),
    /// Effect produced by a future run concurrently with the others.
    Pending(SlotFuture<'a>),
}

fn slot_future<'a>(
    kernel: &'a Kernel,
    slot: &Authorized,
    run: ToolRun<'a>,
    workspace: &std::path::Path,
) -> Result<SlotDispatch<'a>, crate::KernelError> {
    let Some(action) = slot.action.clone() else {
        return Ok(SlotDispatch::Skip);
    };
    let action_id = action.action_id;
    let proposal = &slot.proposal;

    if let Some(host) = kernel.config.plugin_hooks.as_ref() {
        if let Some(effect) = fire_pre_tool_use_for_batch(host, proposal, action_id)? {
            return Ok(SlotDispatch::Ready((effect, 0)));
        }
    }

    let tool_name = proposal.tool.clone();
    let future: SlotFuture<'a> = match run {
        ToolRun::Preset(preset) => {
            return Ok(SlotDispatch::Ready((
                preset_effect(action_id, &tool_name, preset)?,
                0,
            )));
        }
        ToolRun::Local(handler) => Box::pin(async move {
            let started = Instant::now();
            let result = handler.await;
            Ok((
                preset_effect(action_id, &tool_name, result)?,
                elapsed_ms(started),
            ))
        }),
        ToolRun::Executor => {
            let ctx = ExecuteContext::new(kernel.agent_id, action_id, workspace.to_path_buf())
                .with_tool_use_id(proposal.tool_use_id.clone())
                .with_confined_writes(kernel.confines_writes(slot))
                .with_run_scratch(kernel.config.run_scratch.clone());
            Box::pin(async move {
                let started = Instant::now();
                let effect = kernel.execute_with_timeout(&ctx, &action, &tool_name).await;
                Ok((effect, elapsed_ms(started)))
            })
        }
    };
    Ok(SlotDispatch::Pending(future))
}

/// Append the `ToolExecution` entries in proposal order.
fn record_all(
    kernel: &Kernel,
    authorized: Vec<Authorized>,
    per_slot_effect: Vec<Option<(Effect, u64)>>,
) -> Result<Vec<ProcessResult>, crate::KernelError> {
    let mut results = Vec::with_capacity(authorized.len());
    for (slot, effect) in authorized.into_iter().zip(per_slot_effect) {
        let executed = match (slot.action.clone(), effect) {
            (Some(action), Some((effect, duration_ms))) => {
                Some(kernel.record_tool_execution(&slot, action, effect, duration_ms)?)
            }
            // Every allowed slot gets an effect above: a hook block, a
            // preset, or a dispatched future.
            (Some(_), None) => {
                return Err(crate::KernelError::Internal(
                    "allowed verdict did not produce an effect".into(),
                ))
            }
            (None, _) => None,
        };
        results.push(finish_tool_call(slot, executed));
    }

    Ok(results)
}

/// Encode a preset or handler result as the committed effect the executor would have
/// produced, so the agent loop decodes it the same way.
fn preset_effect(
    action_id: ActionId,
    tool: &str,
    preset: PresetToolResult,
) -> Result<Effect, crate::KernelError> {
    let mut result = if preset.is_error {
        ToolResult::failure(tool, preset.content)
    } else {
        ToolResult::success(tool, preset.content)
    };
    result.kind = preset.kind;
    let payload = serde_json::to_vec(&result)
        .map_err(|e| crate::KernelError::Serialization(format!("preset result: {e}")))?;
    Ok(Effect::committed_agreement(action_id, payload))
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
