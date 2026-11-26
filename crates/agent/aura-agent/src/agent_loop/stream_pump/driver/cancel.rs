//! Mid-stream / mid-drain cancellation outcomes: pairing every emitted
//! `tool_use` with a real or `[CANCELLED]` tool_result.

use std::collections::HashSet;

use aura_model::Usage;

use crate::types::{ToolCallInfo, ToolCallResult};

use super::super::synthesize::synthesize_response;
use super::super::StreamPumpOutcome;
use super::{IndexedResult, StreamDrive};

const CANCELLED_CONTENT: &str =
    "[CANCELLED] Tool execution was cancelled by the user before the tool returned.";

const CANCELLED_WRITE_CONTENT: &str =
    "[CANCELLED] Tool execution was cancelled by the user before the tool returned. \
     The change may already have been made: re-read the file before editing it again.";

impl StreamDrive<'_> {
    /// Mid-stream cancellation. If the model has already emitted any
    /// `tool_use` blocks we MUST synthesise paired `[CANCELLED]`
    /// tool_results so the downstream `dispatch_streamed_response` →
    /// `push_tool_result_message_with_context` step can close the
    /// Anthropic `tool_use ↔ tool_result` adjacency contract before the
    /// loop breaks. Pre-fix this returned a bare `Cancelled` and the
    /// conversation history ended with an orphaned assistant `tool_use`
    /// block — invalid on the next sampling call.
    /// See `pipeline_cancellation_mid_tool_execution_aborts_loop`.
    pub(super) fn cancelled_mid_stream(&self, model_name: &str) -> StreamPumpOutcome {
        if self.tool_calls_seen.is_empty() {
            return StreamPumpOutcome::Cancelled;
        }
        // Tool futures are only polled in the drain, so none has run yet.
        cancelled_outcome(&self.snapshot(model_name), Vec::new(), false)
    }

    pub(super) fn snapshot<'s>(&'s self, model_name: &'s str) -> CancelledSnapshot<'s> {
        CancelledSnapshot {
            text_chunks: &self.text_chunks,
            thinking_chunks: &self.thinking_chunks,
            tool_calls_seen: &self.tool_calls_seen,
            spawned_indices: &self.spawned_indices,
            end_turn: self.end_turn,
            usage: &self.usage,
            model_name,
        }
    }
}

/// Build a `Completed` outcome that carries `[CANCELLED]` tool_results
/// for every emitted tool_use that did NOT yet produce a paired
/// result. Drains `cached_pairs` + `spawned_pairs` first (already-
/// resolved entries keep their real result), then synthesises
/// `stop_loop = true` cancelled entries for the rest so the downstream
/// dispatcher breaks the loop on the same iteration.
///
/// Returning `Completed` (instead of `Cancelled`) is deliberate: the
/// sampling driver's `Completed` arm runs
/// `iteration::accumulate_response` (pushes the assistant message with
/// the seen tool_use blocks) and `dispatch_streamed_response` (pushes
/// the paired tool_result-bearing user message). Routing through that
/// path is the only way to satisfy Anthropic's `tool_use ↔ tool_result`
/// adjacency contract on a mid-tool cancel — a bare `Cancelled` would
/// leave an orphaned assistant `tool_use` block at the tail of the
/// transcript and the next sampling call would be structurally invalid.
/// Phase 8 wrapper around the immutable observations [`super::drive_stream`]
/// accumulated before a mid-flight cancellation. Bundles the four
/// `&[…]` slices plus the response-synthesis state needed to fold a
/// partial transcript into a [`StreamPumpOutcome::Completed`] with
/// `[CANCELLED]` tool_results. Owned `spawned_pairs` stays a
/// positional argument so it can be moved into the merge step.
pub(super) struct CancelledSnapshot<'a> {
    text_chunks: &'a [String],
    thinking_chunks: &'a [(String, Option<String>)],
    tool_calls_seen: &'a [ToolCallInfo],
    spawned_indices: &'a [usize],
    end_turn: Option<bool>,
    usage: &'a Usage,
    model_name: &'a str,
}

/// `started` says whether the pending calls may have begun running, in
/// which case an unfinished file change is reported as possibly applied.
pub(super) fn cancelled_outcome(
    snap: &CancelledSnapshot<'_>,
    mut spawned_pairs: Vec<IndexedResult>,
    started: bool,
) -> StreamPumpOutcome {
    push_cancelled_results(snap, &mut spawned_pairs, started);

    spawned_pairs.sort_by_key(|(idx, _)| *idx);
    let tool_results: Vec<(ToolCallInfo, ToolCallResult)> =
        spawned_pairs.into_iter().map(|(_, pair)| pair).collect();

    let response = synthesize_response(
        snap.text_chunks,
        snap.thinking_chunks,
        snap.tool_calls_seen,
        snap.end_turn,
        None,
        snap.usage,
        snap.model_name,
    );
    StreamPumpOutcome::Completed {
        response,
        tool_results,
    }
}

/// Append a `[CANCELLED]` result for every spawned call without one.
fn push_cancelled_results(
    snap: &CancelledSnapshot<'_>,
    spawned_pairs: &mut Vec<IndexedResult>,
    started: bool,
) {
    let already: HashSet<usize> = spawned_pairs.iter().map(|(idx, _)| *idx).collect();

    let still_pending: HashSet<usize> = snap
        .spawned_indices
        .iter()
        .copied()
        .filter(|idx| !already.contains(idx))
        .collect();

    for idx in still_pending {
        if let Some(call) = snap.tool_calls_seen.get(idx) {
            spawned_pairs.push((idx, (call.clone(), cancelled_result(call, started))));
        }
    }
}

fn cancelled_result(call: &ToolCallInfo, started: bool) -> ToolCallResult {
    let content = if started && crate::helpers::is_write_tool(&call.name) {
        CANCELLED_WRITE_CONTENT
    } else {
        CANCELLED_CONTENT
    };
    ToolCallResult {
        tool_use_id: call.id.clone(),
        content: content.to_string(),
        is_error: true,
        kind: aura_core_types::ToolResultKind::AgentError,
        stop_loop: true,
        image: None,
        file_changes: Vec::new(),
    }
}
