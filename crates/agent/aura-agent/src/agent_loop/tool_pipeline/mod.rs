//! Unified tool-execution pipeline (Phase 4 + Phase 7).
//!
//! Phase 4 collapsed the two pre-existing tool-dispatch entry points
//! (the buffered path's `tool_execution::handle_tool_use` and the
//! pump path's `stream_pump::dispatch::handle_streamed_tool_use`)
//! behind one async function: [`process_tool_results`]. Phase 7
//! then dropped the legacy buffered transport entirely, so today
//! every batch arrives pre-executed from the streaming pump.
//!
//! The batch is still wrapped in [`ToolBatch`]:
//!
//! - [`ToolBatch::PreExecuted`] is `Vec<(ToolCallInfo, ToolCallResult)>`
//!   the pump driver already executed against its
//!   [`futures_util::stream::FuturesOrdered`] drain. The pipeline
//!   takes the pre-executed pairs as a pass-through —
//!   `track_tool_effects` / auto-build / message push still run so
//!   the pump path participates in the same telemetry contract.
//!
//! The enum stays even though there is only one variant today so a
//! future transport (an offline cassette transport for replay, for
//! example) can be slotted in as a fresh variant without rewriting
//! the dispatch tail.
//!
//! Each call site builds a [`ToolEffectCtx`] (executor, event_tx,
//! cancellation_token) for the small set of per-dispatch parameters
//! the pipeline needs. The larger sampling context lives in
//! [`super::transport::SamplingCtx`] one layer up.

use std::collections::HashSet;

use crate::events::AgentLoopEvent;
use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};
use aura_model::{ContentBlock, ModelResponse};
use tokio::sync::mpsc::Sender;
use tracing::warn;

use super::event_sink::emit as emit_event;
use super::{AgentLoop, LoopState};

mod effects;
mod heartbeat;
mod results;
mod stop_dispatch;

use effects::{run_auto_build, track_tool_effects};
pub(super) use heartbeat::{spawn_tool_heartbeat, tool_heartbeat_interval};
use results::emit_and_log_results;
pub(super) use results::push_tool_result_message;
pub(crate) use stop_dispatch::dispatch;
pub(super) use stop_dispatch::partition_circling_duplicate_reads;

/// One batch of tool calls handed to [`process_tool_results`].
///
/// Phase 4 introduced the `Live` / `PreExecuted` split so the two
/// transports could share a single dispatch tail. Phase 7 then
/// dropped the buffered transport entirely (parity proven), so the
/// enum now only ever carries the `PreExecuted` shape produced by
/// the streaming pump. Kept as an enum so a future transport (an
/// offline cassette transport, for example) can be reintroduced as
/// `Live` without rewriting every match arm.
pub(crate) enum ToolBatch {
    /// Pump transport: tool calls already executed inside the
    /// pump driver, paired with their [`ToolCallResult`] in
    /// submission order.
    PreExecuted(Vec<(ToolCallInfo, ToolCallResult)>),
}

impl ToolBatch {
    /// `true` when the batch carries no tool calls.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        match self {
            Self::PreExecuted(v) => v.is_empty(),
        }
    }
}

/// Per-dispatch context handed to [`process_tool_results`].
///
/// Kept small and borrowed — the bigger sampling-time bundle lives
/// in [`super::transport::SamplingCtx`]. `ToolEffectCtx` carries only
/// the per-batch arguments that the tool pipeline actually needs:
///
/// - `executor`: target for [`AgentToolExecutor::execute`] on
///   [`ToolBatch::Live`] batches and for the post-write
///   [`AgentToolExecutor::auto_build_check`] on either batch kind.
/// - `event_tx`: optional [`AgentLoopEvent`] sink for warnings,
///   per-tool start/completed/result events, and the post-write
///   build console block.
pub(crate) struct ToolEffectCtx<'a> {
    pub(crate) executor: &'a dyn AgentToolExecutor,
    pub(crate) event_tx: Option<&'a Sender<AgentLoopEvent>>,
}

/// Result of running [`process_tool_results`].
///
/// Phase 4 collapsed the previously-distinct buffered/pump
/// "should_stop" plumbing into one bool that captures every
/// terminate-loop signal:
///
/// - a non-error `task_done` returning `stop_loop = true`,
/// - any synthesised `[CANCELLED]` tool_result (mid-tool cancel
///   path),
/// - any tool that explicitly signals `stop_loop = true` from the
///   executor side.
///
/// The struct is intentionally one-field so callers can grow it
/// (e.g. Phase 8 may add a `task_completed` flag) without churning
/// every dispatcher.
pub(crate) struct ProcessedToolResults {
    pub(crate) should_stop: bool,
}

/// Pull every [`ContentBlock::ToolUse`] block out of `response`'s
/// assistant message, preserving submission order.
///
/// Phase 4: merged from the previously-duplicated
/// `tool_execution::extract_tool_calls` and
/// `iteration::extract_pending_tools` helpers. Callers that need
/// the `path` argument (e.g. the `max_tokens` synthetic-truncation
/// builder) extract it from each [`ToolCallInfo::input`] directly
/// instead of carrying a parallel `PendingTool` struct.
#[must_use]
pub(crate) fn tool_calls(response: &ModelResponse) -> Vec<ToolCallInfo> {
    response
        .message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => Some(ToolCallInfo {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Run the unified post-`ToolUse` pipeline against `batch` and
/// finalise the per-iteration tool effects on `state`.
///
/// Phase 4 keystone: replaces the previously-divergent
/// `tool_execution::handle_tool_use` (buffered) and
/// `stream_pump::dispatch::handle_streamed_tool_use` (pump) dispatch
/// tails. Both call sites now produce a [`ToolBatch`] and route
/// through this function so `track_tool_effects` / auto-build /
/// `dup_audit` / message-push run uniformly regardless of transport.
///
/// The function:
///
/// 1. Builds the unified "tool_calls + all_results" view, executing
///    [`ToolBatch::Live`] batches through the chunk-guard /
///    circling-read / cache / heartbeat / executor chain. For
///    [`ToolBatch::PreExecuted`] batches the pump driver already ran
///    the tools so the pipeline takes the pre-executed pairs as a
///    pass-through.
/// 2. Runs `track_tool_effects` so the cumulative `had_any_write`
///    latch, file-change journal, repeated-read tracker, and
///    session-read cache all light up the same way on both paths.
/// 3. Triggers auto-build when any successful write landed (with
///    `build_cooldown == 0`) so the dev-loop build feedback loop
///    fires regardless of transport.
/// 4. Emits the per-batch console block and per-tool
///    `ToolCallCompleted` / `ToolResult` events.
/// 5. Pushes a single `Role::User` message carrying the
///    `tool_result` blocks (and any side-message context texts) so
///    the Anthropic `tool_use ↔ tool_result` adjacency contract
///    stays intact.
/// 6. Returns whether the sampling loop should break (the union of
///    every `stop_loop = true` signal in the batch plus the
///    `task_done` success handshake).
pub(crate) async fn process_tool_results(
    state: &mut LoopState,
    agent: &AgentLoop,
    batch: ToolBatch,
    ctx: ToolEffectCtx<'_>,
) -> ProcessedToolResults {
    // Stop-reason parity: when the model signals `ToolUse` but emits
    // no actual tool_use blocks (Phase 3 made this reachable for the
    // pump's MaxTokens-without-pending-tools synthesis path too, see
    // `synthesize.rs::derive_stop_reason`), the loop has nothing to
    // dispatch and must break. Pre-Phase-4 the buffered path's
    // `handle_tool_use` returned early in this case via
    // `execute_and_cache_tools` and the pump's `handle_streamed_tool_use`
    // mirrored it with an explicit `if tool_results.is_empty()`
    // bailout. The unified pipeline preserves that contract here so
    // a no-op `ToolUse` does not spin the sampling loop on a
    // never-executing tool batch.
    if batch.is_empty() {
        return ProcessedToolResults { should_stop: true };
    }

    let prepared = match batch {
        ToolBatch::PreExecuted(pairs) => prepare_pre_executed_batch(pairs),
    };

    let PreparedBatch {
        tool_calls,
        mut all_results,
        mut side_messages,
        blocked_ids,
        cached_ids,
    } = prepared;

    fire_post_tool_use_hooks(agent, &tool_calls, &all_results);

    // Cumulative tracking (steering registry, file-change journal,
    // session-read cache, read-after-write allowances). The Live
    // path's cached-results subset is included here because
    // `prepare_live_batch` folded cached pairs into `all_results`
    // before tracking ran. The pump driver does the same merge
    // before calling into the pipeline so PreExecuted batches reach
    // tracking with the full set.
    //
    // Phase 5: the three previously-threaded `Option<&mut ...>`
    // trackers (repeated-read, session-read-paths,
    // read-after-write-allowances) collapsed into a single
    // `&mut SteeringRegistry` borrow + the still-needed
    // session-read-paths / allowances borrows. The registry handles
    // the per-source telemetry; the path-cache + allowance maps stay
    // on `LoopState` because the circling-read gate consults them
    // directly.
    let any_write_success = track_tool_effects(&tool_calls, &all_results, state);
    let repeated_edit_failure =
        guard_failed_edits(&tool_calls, &all_results, state, &mut side_messages);
    state.decisions.extend(super::decisions::from_tool_results(
        &tool_calls,
        &all_results,
    ));
    state.observe_task_results(&tool_calls, &all_results, ctx.event_tx);

    if any_write_success && state.build_cooldown == 0 {
        auto_build_after_writes(state, agent, ctx.executor, &all_results, &mut side_messages).await;
    }

    // Phase B latch: any successful write flips both `had_any_write`
    // (per-batch) and `had_any_file_write` (cumulative). The pump
    // path historically duplicated this assignment inside
    // `handle_streamed_tool_use`; the unified pipeline runs it once
    // here regardless of transport.
    if state.had_any_write {
        state.had_any_file_write = true;
    }

    let reported = std::mem::take(&mut state.reported_tool_results);
    emit_and_log_results(
        ctx.event_tx,
        &tool_calls,
        &all_results,
        &cached_ids,
        &blocked_ids,
        &reported,
    );

    mark_task_done(state, &tool_calls, &all_results);
    warn_compaction_structural(&all_results);

    let should_stop = repeated_edit_failure.is_some() || all_results.iter().any(|r| r.stop_loop);

    // Drain `all_results` into the trailing user message; carry side
    // messages (chunk-guard warnings + auto-build output) as
    // post-tool_result text blocks. Single dup_audit pre/post pair
    // covers both transports now that the message-push lives here.
    let pushed_results = std::mem::take(&mut all_results);
    push_tool_result_message(&mut state.messages, pushed_results, side_messages);

    if let Some(message) = repeated_edit_failure {
        report_repeated_edit_failure(state, ctx.event_tx, message);
    }

    ProcessedToolResults { should_stop }
}

// Phase 8 + Phase 10 carve-out 5a: `PreToolUse` mid-flight
// cancellation now lives in the kernel's
// `aura_agent_kernel::kernel::tools::{single,batch}` pre-
// dispatch site — a `HookOutcome::Block` there aborts the
// executor and surfaces a synthetic failed effect tagged
// `tool_call_blocked_by_hook`. The streaming pump's
// `process_tool_results` runs AFTER the executor (PreExecuted
// batch), so re-firing `PreToolUse` here would double-fire
// the hook and is intentionally OMITTED.
//
// `PostToolUse` continues to fire here (observer-only) because
// it is by definition post-dispatch and the streaming pump
// owns the batch of `(call, result)` pairs at this site.
fn fire_post_tool_use_hooks(
    agent: &AgentLoop,
    tool_calls: &[ToolCallInfo],
    all_results: &[ToolCallResult],
) {
    if let Some(host) = agent.config.plugin_hooks.as_ref() {
        if !host.is_empty(aura_plugin_hooks::HookEvent::PostToolUse) {
            for r in all_results {
                let tc_name = tool_calls
                    .iter()
                    .find(|t| t.id == r.tool_use_id)
                    .map_or("", |t| t.name.as_str());
                let status = if r.is_error { "error" } else { "ok" };
                let summary: String = r.content.chars().take(256).collect();
                host.fire_post_tool_use(tc_name, &r.tool_use_id, status, &summary);
            }
        }
    }
}

async fn auto_build_after_writes(
    state: &mut LoopState,
    agent: &AgentLoop,
    executor: &dyn AgentToolExecutor,
    all_results: &[ToolCallResult],
    side_messages: &mut Vec<String>,
) {
    let changed: Vec<String> = all_results
        .iter()
        .flat_map(|r| r.file_changes.iter().map(|c| c.path.clone()))
        .collect();
    if let Some(build_text) = run_auto_build(
        &agent.config,
        executor,
        &mut state.build_cooldown,
        state.build_baseline.as_ref(),
        &changed,
        &mut state.decisions,
    )
    .await
    {
        side_messages.push(build_text);
    }
}

fn mark_task_done(
    state: &mut LoopState,
    tool_calls: &[ToolCallInfo],
    all_results: &[ToolCallResult],
) {
    let task_done_success = tool_calls.iter().any(|tc| {
        tc.name == "task_done"
            && all_results
                .iter()
                .any(|r| r.tool_use_id == tc.id && !r.is_error && r.stop_loop)
    });
    if task_done_success {
        state.task_done_completed = true;
    }
}

fn warn_compaction_structural(all_results: &[ToolCallResult]) {
    for result in all_results
        .iter()
        .filter(|r| r.kind == aura_core_types::ToolResultKind::CompactionStructural)
    {
        warn!(
            target: "compaction",
            tool_use_id = %result.tool_use_id,
            result_len = result.content.len(),
            "Rejected compacted/redacted tool input without incrementing consecutive errors"
        );
    }
}

fn report_repeated_edit_failure(
    state: &mut LoopState,
    event_tx: Option<&Sender<AgentLoopEvent>>,
    message: String,
) {
    state.result.stalled = true;
    state.result.llm_error = Some(message.clone());
    emit_event(
        event_tx,
        AgentLoopEvent::Error {
            code: "tool_no_progress".to_string(),
            message,
            recoverable: true,
        },
    );
}

/// Reads between retries do not make a failed edit new. A successful mutation
/// does: reset the tracker so a genuinely changed file gets a fresh budget.
fn guard_failed_edits(
    calls: &[ToolCallInfo],
    results: &[ToolCallResult],
    state: &mut LoopState,
    side_messages: &mut Vec<String>,
) -> Option<String> {
    for result in results {
        let Some(call) = calls.iter().find(|call| call.id == result.tool_use_id) else {
            continue;
        };
        if !result.is_error && !result.file_changes.is_empty() {
            state.failed_edit_attempts.clear();
            continue;
        }
        if call.name != "edit_file"
            || !result.is_error
            || result.kind == aura_core_types::ToolResultKind::CompactionStructural
        {
            continue;
        }
        let key = format!("{}\n{}", call.input, result.content);
        let steering = aura_config::agent().steering;
        let count = state.failed_edit_attempts.entry(key).or_default();
        *count = count.saturating_add(1);
        let count = *count;
        if count == steering.failed_edit_warn_after {
            side_messages.push(format!("The identical edit has failed {count} times. Do not repeat it unchanged. Read the current file, inspect the error, and use a different matching edit; if blocked, explain the blocker."));
            state.decide(super::decisions::failed_edit_guard(call, count, false));
        }
        if count >= steering.failed_edit_stop_after {
            state.decide(super::decisions::failed_edit_guard(call, count, true));
            return Some(format!("Aura stopped a repeated failed edit loop after {count} identical failures. No successful edit was made by those attempts. Review the tool error and current file before restarting."));
        }
    }
    None
}

/// Intermediate view assembled per batch before the common
/// tracking / auto-build / emit / push steps fire. Held by value
/// inside [`process_tool_results`] so the per-batch-kind logic stays
/// in a single struct and the post-prepare chain is identical.
struct PreparedBatch {
    tool_calls: Vec<ToolCallInfo>,
    all_results: Vec<ToolCallResult>,
    side_messages: Vec<String>,
    blocked_ids: HashSet<String>,
    cached_ids: HashSet<String>,
}

fn prepare_pre_executed_batch(pairs: Vec<(ToolCallInfo, ToolCallResult)>) -> PreparedBatch {
    let mut tool_calls: Vec<ToolCallInfo> = Vec::with_capacity(pairs.len());
    let mut all_results: Vec<ToolCallResult> = Vec::with_capacity(pairs.len());
    for (call, result) in pairs {
        tool_calls.push(call);
        all_results.push(result);
    }
    PreparedBatch {
        tool_calls,
        all_results,
        side_messages: Vec::new(),
        // The pump driver knows which entries were cached or
        // circling-blocked but does not currently surface that
        // metadata into the dispatch tail; both sets stay empty so
        // the console block labels everything as `executor`. A
        // follow-up can plumb the per-entry classification through
        // `StreamPumpOutcome` so the console block matches the
        // pre-Phase-7 buffered shape.
        blocked_ids: HashSet::new(),
        cached_ids: HashSet::new(),
    }
}

#[cfg(test)]
mod track_tool_effects_tests;
