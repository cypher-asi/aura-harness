//! Tool-call dispatch: admitting each closed `tool_use` block into the
//! in-flight set (steering gate, per-run cache, executor spawn) and the
//! per-tool future that reports its result.

use std::time::Duration;

use tokio::time::timeout;

use crate::agent_loop::LoopState;
use crate::events::AgentLoopEvent;
use crate::session::input_queue::InputQueue;
use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};
use crate::AgentError;

use super::super::{emit_event, StreamPumpOutcome};
use super::{StreamDrive, ToolFuture};

impl<'a> StreamDrive<'a> {
    pub(super) fn on_invalid_tool_use(&mut self, id: String, name: String, reason: String) {
        self.open_tool = None;
        let call = ToolCallInfo {
            id: id.clone(),
            name,
            input: serde_json::json!({}),
        };
        let index = self.tool_calls_seen.len();
        self.tool_calls_seen.push(call.clone());
        self.push_preset(
            index,
            call,
            "invalid_tool_use",
            ToolCallResult::error(id, reason),
        );
    }

    /// Handle one `OutputItemDone(ToolUse)` event: circling-read gate,
    /// per-run cache lookup, spawn-or-serve, and per-tool-result
    /// input-queue drain. Returns `Some(StreamPumpOutcome)` only when an
    /// invariant-violation early-return must propagate to
    /// [`super::drive_stream`] (e.g. the per-run cache partition contract
    /// is broken). On the happy path returns `None` and pushes the call
    /// into the FIFO.
    pub(super) async fn on_tool_use(
        &mut self,
        call: ToolCallInfo,
        state: &mut LoopState,
    ) -> Option<StreamPumpOutcome> {
        // `announced` is set when `ToolStart` already went out at block start.
        let announced = self.open_tool.take().is_some_and(|tool| tool.id == call.id);
        let submission_index = self.tool_calls_seen.len();
        self.tool_calls_seen.push(call.clone());
        self.announce_tool_use(&call, announced);

        // Circling read gate: before cache lookup or executor spawn,
        // reject duplicate `read_file` paths once the steering layer
        // has latched a read-only loop. This keeps the streaming pump
        // in lockstep with the buffered dispatcher.
        let single = std::slice::from_ref(&call);
        let (blocked_reads, allowed_calls) =
            crate::agent_loop::tool_pipeline::partition_circling_duplicate_reads(single, state);
        if let Some(blocked) = blocked_reads.into_iter().next() {
            let decision = crate::agent_loop::decisions::implement_now_block(&call, state);
            state.decide(decision);
            self.push_preset(submission_index, call, "steering_block", blocked);
            return None;
        }

        if let Some(outcome) = self.submit_allowed(call, &allowed_calls, submission_index, state) {
            return Some(outcome);
        }
        drain_input_queue(self.ctx.input_queue, state).await;
        None
    }

    /// Layer E.4: emit ToolStart (unless the block start already did) and
    /// a final ToolInputSnapshot carrying the full parsed input, so chat
    /// UX sees the same event sequence as the buffered streaming path.
    fn announce_tool_use(&self, call: &ToolCallInfo, announced: bool) {
        let event_tx = self.ctx.event_tx;
        if !announced {
            emit_event(
                event_tx,
                AgentLoopEvent::ToolStart {
                    id: call.id.clone(),
                    name: call.name.clone(),
                },
            );
        }
        emit_event(
            event_tx,
            AgentLoopEvent::ToolInputSnapshot {
                id: call.id.clone(),
                name: call.name.clone(),
                input: call.input.to_string(),
            },
        );
    }

    /// Layer E.4 tool-result cache: consult the per-run cache before
    /// spawning the future. On a hit the cached [`ToolCallResult`] is
    /// handed to the executor as a preset, so the kernel still records
    /// and gates the call but the tool itself does NOT run again (codex
    /// parity for read-only tools).
    ///
    /// Cache invariant (Rule 4.1 / 4.3): `split_cached` partitions
    /// `allowed_calls` (a single-element slice here) into
    /// `cached_results` and `uncached_calls` such that every input
    /// call ends up in exactly one bucket. So when either bucket is
    /// non-empty its `into_iter().next()` MUST yield an element; a
    /// `None` here is a true partition invariant violation. Surface
    /// it as a fatal `LlmCallError` via `AgentError::Internal` rather
    /// than panicking — the `Error` outcome is folded into
    /// `LlmCallError::Fatal` upstream in `sampling::run_sampling_request`.
    fn submit_allowed(
        &mut self,
        call: ToolCallInfo,
        allowed_calls: &[ToolCallInfo],
        submission_index: usize,
        state: &mut LoopState,
    ) -> Option<StreamPumpOutcome> {
        let (cached_results, uncached_calls) =
            crate::agent_loop::tool_execution::split_cached(allowed_calls, &state.tool_cache);
        if !cached_results.is_empty() {
            let Some(first) = cached_results.into_iter().next() else {
                return Some(StreamPumpOutcome::Error(AgentError::Internal(
                    "stream pump cache invariant: cached_results was non-empty but yielded no element \
                     (split_cached partition contract broken)"
                        .to_string(),
                )));
            };
            state.decide(crate::agent_loop::decisions::cache_hit(&call));
            self.push_preset(submission_index, call, "cache", first);
        } else if !uncached_calls.is_empty() {
            let Some(first) = uncached_calls.into_iter().next() else {
                return Some(StreamPumpOutcome::Error(AgentError::Internal(
                    "stream pump cache invariant: uncached_calls was non-empty but yielded no element \
                     (split_cached partition contract broken)"
                        .to_string(),
                )));
            };
            self.spawned_indices.push(submission_index);
            self.in_flight.push(spawn_tool_with_timeout(
                submission_index,
                self.ctx.executor,
                first,
                None,
                self.ctx.config.per_tool_timeout,
                self.ctx.event_tx,
            ));
        }
        None
    }

    /// Submit a call whose result the loop already has; see
    /// `preset_indices`.
    fn push_preset(
        &mut self,
        index: usize,
        call: ToolCallInfo,
        source: &'static str,
        result: ToolCallResult,
    ) {
        self.spawned_indices.push(index);
        self.preset_indices.push(index);
        self.in_flight.push(spawn_tool_with_timeout(
            index,
            self.ctx.executor,
            call,
            Some((source, result)),
            self.ctx.config.per_tool_timeout,
            self.ctx.event_tx,
        ));
    }
}

/// E.3 input-drain granularity: once per `OutputItemDone(tool_use)`.
/// See module-level docs.
async fn drain_input_queue(input_queue: Option<&InputQueue>, state: &mut LoopState) {
    if let Some(queue) = input_queue {
        if queue.has_pending() {
            let drained = queue.drain().await;
            if !drained.is_empty() {
                crate::agent_loop::turn::apply_user_inputs_to_messages(
                    &mut state.messages,
                    drained,
                );
            }
        }
    }
}

/// Tell the client a call finished the moment it does. Calls of one batch
/// run together, and one waiting on an approval must not keep the others
/// looking unfinished.
fn report_result(
    event_tx: Option<&tokio::sync::mpsc::Sender<AgentLoopEvent>>,
    call: &ToolCallInfo,
    result: &ToolCallResult,
) {
    emit_event(
        event_tx,
        AgentLoopEvent::ToolCallCompleted {
            tool_use_id: result.tool_use_id.clone(),
            tool_name: call.name.clone(),
            input: call.input.clone(),
            is_error: result.is_error,
        },
    );
    emit_event(
        event_tx,
        AgentLoopEvent::ToolResult {
            tool_use_id: result.tool_use_id.clone(),
            tool_name: call.name.clone(),
            content: result.content.clone(),
            is_error: result.is_error,
            image: result.image.clone(),
        },
    );
}

/// `preset` is a result the loop already has for `call`, with the reason
/// it has it; the executor records and gates it instead of running the
/// tool.
fn spawn_tool_with_timeout<'a>(
    index: usize,
    executor: &'a dyn AgentToolExecutor,
    call: ToolCallInfo,
    preset: Option<(&'static str, ToolCallResult)>,
    per_tool_timeout: Duration,
    event_tx: Option<&'a tokio::sync::mpsc::Sender<AgentLoopEvent>>,
) -> ToolFuture<'a> {
    Box::pin(async move {
        let call_clone = call.clone();
        let id = call_clone.id.clone();
        let name = call_clone.name.clone();
        let fut = match preset {
            Some((source, result)) => executor.spawn_preset(call_clone, source, result),
            None => executor.spawn_tool_call(call_clone),
        };
        let result = match timeout(per_tool_timeout, fut).await {
            Ok(result) => result,
            Err(_elapsed) => {
                let elapsed_ms = per_tool_timeout.as_millis();
                ToolCallResult::error(
                    id,
                    format!("tool '{name}' timed out after {elapsed_ms}ms (per_tool_timeout)"),
                )
            }
        };
        report_result(event_tx, &call, &result);
        (index, (call, result))
    })
}
