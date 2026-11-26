//! Post-stream tail of one attempt: response synthesis, the in-flight
//! tool drain (with cancellation and write settling) and the final
//! `Completed` outcome with the per-run cache refresh.

use std::time::Duration;

use aura_model::{ModelResponse, StopReason};
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::agent_loop::LoopState;
use crate::events::AgentLoopEvent;
use crate::types::{ToolCallInfo, ToolCallResult};

use super::super::synthesize::synthesize_response;
use super::super::StreamPumpOutcome;
use super::cancel::cancelled_outcome;
use super::{IndexedResult, StreamDrive, ToolFuture};

/// How long a cancel waits for file-changing calls that are already in
/// flight. A write that has reached the filesystem runs to completion even
/// when its future is dropped, so reporting it as cancelled would tell the
/// model the file is unchanged when it isn't. File operations finish in
/// milliseconds; anything still pending after this (say, waiting on an
/// approval) is reported as possibly applied.
const WRITE_SETTLE_GRACE: Duration = Duration::from_secs(2);

impl StreamDrive<'_> {
    pub(super) fn synthesize(
        &self,
        stop_reason: Option<StopReason>,
        model_name: &str,
    ) -> ModelResponse {
        let response = synthesize_response(
            &self.text_chunks,
            &self.thinking_chunks,
            &self.tool_calls_seen,
            self.end_turn,
            stop_reason,
            &self.usage,
            model_name,
        );
        if let Some(on_response) = self.ctx.on_response {
            on_response(&response);
        }
        response
    }

    /// Drain the in-flight calls (codex `drain_in_flight`). Honours
    /// cancellation: a token fired during drain still aborts before we
    /// mutate `state.messages` in the caller.
    ///
    /// Calls are collected as they finish, each tagged with its
    /// submission index, and sorted afterwards so the final
    /// `tool_results` vec mirrors the order the model emitted the
    /// corresponding `OutputItemDone(tool_use)` events. Collecting out of
    /// order matters on cancel: a call that already ran (an edit that
    /// landed) keeps its real result even when an earlier call is still
    /// waiting, e.g. on an approval.
    pub(super) async fn drain_in_flight(
        &mut self,
        state: &mut LoopState,
        model_name: &str,
    ) -> Result<Vec<IndexedResult>, StreamPumpOutcome> {
        let mut spawned_pairs: Vec<IndexedResult> = Vec::new();
        loop {
            let maybe_next = drain_next(&mut self.in_flight, self.ctx.cancellation_token).await;
            match maybe_next {
                DrainStep::Cancelled => {
                    return Err(self
                        .cancelled_mid_drain(state, model_name, spawned_pairs)
                        .await)
                }
                DrainStep::Done => return Ok(spawned_pairs),
                DrainStep::Result(indexed) => spawned_pairs.push(*indexed),
            }
        }
    }

    /// Mid-drain cancellation. Drop the still-pending `in_flight`
    /// futures (cancellation-safe — they were wrapped in
    /// `tokio::time::timeout` and the executor borrow ends here) and
    /// synthesise `[CANCELLED]` tool_results for every `tool_calls_seen`
    /// entry that has NOT yet produced a paired result. The downstream
    /// dispatch tail (`dispatch_streamed_response` →
    /// `push_tool_result_message_with_context`) then closes the
    /// Anthropic `tool_use ↔ tool_result` adjacency contract before the
    /// loop breaks on the synthesised `stop_loop = true` flag. Pre-fix
    /// this returned a bare `Cancelled` and let `executor.execute(...)`
    /// run to natural completion — the regression contract is
    /// `pipeline_cancellation_mid_tool_execution_aborts_loop`.
    async fn cancelled_mid_drain(
        &mut self,
        state: &mut LoopState,
        model_name: &str,
        mut spawned_pairs: Vec<IndexedResult>,
    ) -> StreamPumpOutcome {
        settle_pending_writes(
            &mut self.in_flight,
            &self.tool_calls_seen,
            &self.spawned_indices,
            &mut spawned_pairs,
            WRITE_SETTLE_GRACE,
        )
        .await;
        let snap = self.snapshot(model_name);
        note_reported(state, self.ctx.event_tx, &spawned_pairs);
        cancelled_outcome(&snap, spawned_pairs, true)
    }

    pub(super) fn finish(
        self,
        state: &mut LoopState,
        response: ModelResponse,
        mut spawned_pairs: Vec<IndexedResult>,
    ) -> StreamPumpOutcome {
        note_reported(state, self.ctx.event_tx, &spawned_pairs);
        spawned_pairs.sort_by_key(|(idx, _)| *idx);
        let tool_results: Vec<(ToolCallInfo, ToolCallResult)> =
            spawned_pairs.into_iter().map(|(_, pair)| pair).collect();
        self.refresh_cache(state, &tool_results);
        StreamPumpOutcome::Completed {
            response,
            tool_results,
        }
    }

    /// Layer E.4: refresh the per-run cache with any newly-executed
    /// results. Mirrors the buffered path's
    /// `tool_execution::handle_tool_use` → `update_cache` step so the
    /// pump path participates in the same memoization /
    /// invalidate-on-write contract. The "uncached" set is the tools
    /// the executor actually ran (preset results were already known at
    /// submission time).
    fn refresh_cache(
        &self,
        state: &mut LoopState,
        tool_results: &[(ToolCallInfo, ToolCallResult)],
    ) {
        let spawned_calls: Vec<ToolCallInfo> = self
            .spawned_indices
            .iter()
            .filter(|i| !self.preset_indices.contains(i))
            .filter_map(|i| self.tool_calls_seen.get(*i).cloned())
            .collect();
        let spawned_results: Vec<ToolCallResult> = tool_results
            .iter()
            .filter(|(c, _)| spawned_calls.iter().any(|sc| sc.id == c.id))
            .map(|(_, r)| r.clone())
            .collect();
        crate::agent_loop::tool_execution::update_cache(
            &mut state.tool_cache,
            &spawned_calls,
            &spawned_results,
        );
    }
}

enum DrainStep {
    // Boxed: `ToolCallResult` carries an optional screenshot image, so
    // the pair is large relative to the unit `Done` / `Cancelled`
    // variants. Boxing keeps the enum small (clippy::large_enum_variant).
    Result(Box<IndexedResult>),
    Done,
    Cancelled,
}

async fn drain_next<'a>(
    in_flight: &mut FuturesUnordered<ToolFuture<'a>>,
    cancellation_token: Option<&CancellationToken>,
) -> DrainStep {
    if in_flight.is_empty() {
        return DrainStep::Done;
    }
    if let Some(token) = cancellation_token {
        tokio::select! {
            biased;
            () = token.cancelled() => DrainStep::Cancelled,
            next = in_flight.next() => match next {
                Some(pair) => DrainStep::Result(Box::new(pair)),
                None => DrainStep::Done,
            }
        }
    } else {
        match in_flight.next().await {
            Some(pair) => DrainStep::Result(Box::new(pair)),
            None => DrainStep::Done,
        }
    }
}

/// On cancel, keep draining `in_flight` for up to `grace` while any
/// spawned file-changing call is still without a result, so the ones that
/// land keep their real results.
async fn settle_pending_writes(
    in_flight: &mut FuturesUnordered<ToolFuture<'_>>,
    tool_calls_seen: &[ToolCallInfo],
    spawned_indices: &[usize],
    spawned_pairs: &mut Vec<IndexedResult>,
    grace: Duration,
) {
    let deadline = tokio::time::Instant::now() + grace;
    loop {
        let write_pending = spawned_indices.iter().any(|idx| {
            tool_calls_seen
                .get(*idx)
                .is_some_and(|call| crate::helpers::is_write_tool(&call.name))
                && !spawned_pairs.iter().any(|(done, _)| done == idx)
        });
        if !write_pending {
            return;
        }
        match tokio::time::timeout_at(deadline, in_flight.next()).await {
            Ok(Some(pair)) => spawned_pairs.push(pair),
            Ok(None) | Err(_) => return,
        }
    }
}

/// Remember which results the tool futures already reported, so the
/// dispatch tail does not report them again.
fn note_reported(
    state: &mut LoopState,
    event_tx: Option<&tokio::sync::mpsc::Sender<AgentLoopEvent>>,
    pairs: &[IndexedResult],
) {
    if event_tx.is_some() {
        state
            .reported_tool_results
            .extend(pairs.iter().map(|(_, (call, _))| call.id.clone()));
    }
}
