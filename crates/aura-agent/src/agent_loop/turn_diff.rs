//! Per-iteration net file-op accumulator.
//!
//! Port of codex's `TurnDiffTracker`
//! ([codex-rs/core/src/turn_diff_tracker.rs:16](https://github.com/.../codex-rs/core/src/turn_diff_tracker.rs))
//! adapted to aura's `write_file` / `edit_file` / `delete_file` tool
//! surface (the codex tracker is shaped around its single-tool patch
//! envelope; aura keeps the granular write tools after Layer 0.4).
//!
//! Phase 1.A foundation for [`super::continuation`]: the continuation
//! runtime needs path-level data — not just a `had_any_file_write:
//! bool` — to detect "no forward motion this turn" and to compute a
//! blocker_signature for the codex-style blocked-after-3 audit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The net effect of a single iteration's write-tool calls on one file.
///
/// Later calls in the same iteration override earlier ones — e.g. a
/// `delete_file` after a `write_file` collapses to `Deleted`. Codex's
/// tracker does the same coalescing on its single-envelope patch ops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileOp {
    Created,
    Modified { bytes_written: usize },
    Deleted,
}

/// A write-tool call (`write_file` / `edit_file` / `delete_file`)
/// that returned `is_error = true` this iteration. Captured by
/// [`super::tool_pipeline::track_tool_effects`] alongside successful
/// writes so the out-of-loop continuation runtime can distinguish
/// "no write attempted" from "write attempted but rejected" and feed
/// the rejected attempts back to the model as steering text.
///
/// Priority A motivation: pre-A, a turn where the model emitted an
/// `edit_file` whose needle missed was indistinguishable from a turn
/// that called no write tool at all — both produced an empty
/// [`TurnDiff::writes`] and both advanced the no-write streak with
/// the same `Nudge` continuation body. The model never saw its own
/// recent tool-layer rejections in the steering text, so it kept
/// guessing the same wrong needle until `max_continuation_turns`
/// tripped `TaskBlocked` (the doom-loop symptom that re-surfaced in
/// the latest automation run).
#[derive(Debug, Clone)]
pub(crate) struct FailedWriteAttempt {
    /// Originating tool name (one of [`crate::constants::WRITE_TOOLS`]).
    pub(crate) tool: String,
    /// Best-effort `path` argument lifted from the tool input. `None`
    /// when the input did not carry a `path` field (e.g. a multi-file
    /// writer) or when the JSON parse failed defensively.
    pub(crate) target_path: Option<String>,
    /// First ~200 characters of the error body — enough for the model
    /// to see *what* went wrong without quoting the full executor
    /// trace back into the continuation envelope. The full error body
    /// is still on `state.messages` as the tool_result block.
    pub(crate) error_snippet: String,
}

/// Maximum bytes of `error_text` echoed back to the model via
/// [`FailedWriteAttempt::error_snippet`]. Keeps the continuation
/// envelope compact so a long executor stack-trace cannot itself
/// blow the next sampling request's input cap. 200 chars typically
/// covers the `path not found: …` / `None of the … needle line(s)
/// match …` headline that downstream tools emit.
pub(crate) const FAILED_WRITE_SNIPPET_MAX_CHARS: usize = 200;

/// Net file-op map for the current iteration.
///
/// Reset at the top of every iteration by the agent loop; consulted by
/// `continuation::ContinuationState::on_iteration_end` to decide
/// whether the turn produced forward motion.
#[derive(Debug, Default, Clone)]
pub(crate) struct TurnDiff {
    writes: HashMap<PathBuf, FileOp>,
    /// Write-tool calls (`write_file` / `edit_file` / `delete_file`)
    /// that returned `is_error = true` this turn, in the order the
    /// tool dispatcher saw them. Cleared by [`Self::reset`] together
    /// with [`Self::writes`] so the per-iteration scoping invariant
    /// (Phase 1.A) continues to hold.
    failed_write_attempts: Vec<FailedWriteAttempt>,
}

impl TurnDiff {
    /// Record a `write_file` on a path that did not exist before this
    /// iteration. Overrides any prior op on the same path (last-write-
    /// wins within an iteration).
    pub(crate) fn record_create(&mut self, path: PathBuf) {
        self.writes.insert(path, FileOp::Created);
    }

    /// Record a `write_file` / `edit_file` on an existing path. If the
    /// path was previously recorded as `Created` or `Modified` within
    /// this iteration, the byte count is summed onto the existing
    /// `Modified` entry (a `Created` entry is left as `Created` —
    /// is_empty() and the per-path coarse signal don't distinguish).
    /// `Deleted` is preserved (a delete-then-modify is unusual but
    /// the delete is the stronger signal).
    pub(crate) fn record_modify(&mut self, path: PathBuf, bytes: usize) {
        self.writes
            .entry(path)
            .and_modify(|op| {
                if let FileOp::Modified { bytes_written } = op {
                    *bytes_written = bytes_written.saturating_add(bytes);
                }
            })
            .or_insert(FileOp::Modified {
                bytes_written: bytes,
            });
    }

    /// Record a `delete_file`. Overrides any prior op on the same path
    /// — a create-then-delete in one iteration collapses to a deletion.
    pub(crate) fn record_delete(&mut self, path: PathBuf) {
        self.writes.insert(path, FileOp::Deleted);
    }

    /// Returns true when no write/edit/delete landed this iteration.
    /// This is the per-iteration "no forward motion" signal consumed
    /// by `ContinuationState::on_iteration_end` (Phase 1.B).
    ///
    /// NOTE: this only considers *successful* writes (codex parity).
    /// Failed write attempts live on [`Self::failed_write_attempts`]
    /// and are surfaced through a separate channel so the no-write
    /// streak distinguishes "tried and was rejected" from "did not
    /// try" (Priority A).
    pub(crate) fn is_empty(&self) -> bool {
        self.writes.is_empty()
    }

    /// Append one `is_error = true` write-tool attempt. Called by
    /// [`super::tool_pipeline::track_tool_effects`] (both the
    /// buffered and streaming-pump paths fan into the same recorder
    /// via [`super::tool_pipeline::track_tool_effects_public`]).
    pub(crate) fn record_failed_write(&mut self, attempt: FailedWriteAttempt) {
        self.failed_write_attempts.push(attempt);
    }

    /// Snapshot of failed-write attempts recorded this iteration, in
    /// submission order. Consumed by the goal-runtime turn-stop hook
    /// to thread the entries through into
    /// [`crate::session::goal_runtime::ContinuationState`]'s
    /// session-scoped buffer before [`Self::reset`] wipes them at
    /// the top of the next iteration.
    pub(crate) fn failed_write_attempts(&self) -> &[FailedWriteAttempt] {
        &self.failed_write_attempts
    }

    /// Iterate over the paths touched this iteration. Reserved for the
    /// blocker_signature computation in a future Phase 1.B follow-up
    /// (the integration is currently best-effort).
    #[allow(dead_code)]
    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.writes.keys().map(PathBuf::as_path)
    }

    /// Clear all entries. Called at the top of each iteration by the
    /// agent loop so the diff scopes to the iteration just executed.
    pub(crate) fn reset(&mut self) {
        self.writes.clear();
        self.failed_write_attempts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_diff_create_then_modify_keeps_modified_with_summed_bytes() {
        let mut diff = TurnDiff::default();
        let path = PathBuf::from("src/lib.rs");
        diff.record_modify(path.clone(), 100);
        diff.record_modify(path.clone(), 50);
        assert!(!diff.is_empty());
        let op = diff.writes.get(&path).expect("entry must exist");
        assert_eq!(op, &FileOp::Modified { bytes_written: 150 });
    }

    #[test]
    fn turn_diff_create_followed_by_modify_keeps_created() {
        let mut diff = TurnDiff::default();
        let path = PathBuf::from("src/new.rs");
        diff.record_create(path.clone());
        diff.record_modify(path.clone(), 42);
        assert_eq!(diff.writes.get(&path), Some(&FileOp::Created));
    }

    #[test]
    fn turn_diff_delete_overrides_prior_op() {
        let mut diff = TurnDiff::default();
        let path = PathBuf::from("src/gone.rs");
        diff.record_create(path.clone());
        diff.record_modify(path.clone(), 99);
        diff.record_delete(path.clone());
        assert_eq!(diff.writes.get(&path), Some(&FileOp::Deleted));
    }

    #[test]
    fn turn_diff_reset_clears_all() {
        let mut diff = TurnDiff::default();
        diff.record_create(PathBuf::from("a.rs"));
        diff.record_modify(PathBuf::from("b.rs"), 10);
        diff.record_delete(PathBuf::from("c.rs"));
        assert!(!diff.is_empty());
        diff.reset();
        assert!(diff.is_empty());
        assert_eq!(diff.paths().count(), 0);
    }

    #[test]
    fn turn_diff_is_empty_after_default() {
        let diff = TurnDiff::default();
        assert!(diff.is_empty());
    }

    #[test]
    fn turn_diff_paths_iterates_all_recorded() {
        let mut diff = TurnDiff::default();
        diff.record_create(PathBuf::from("a.rs"));
        diff.record_modify(PathBuf::from("b.rs"), 1);
        let mut paths: Vec<_> = diff.paths().map(Path::to_path_buf).collect();
        paths.sort();
        assert_eq!(paths, vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")]);
    }

    // ----------------------------------------------------------------
    // Priority A: failed-write attempt recording
    // ----------------------------------------------------------------

    fn mk_attempt(tool: &str, path: Option<&str>, snippet: &str) -> FailedWriteAttempt {
        FailedWriteAttempt {
            tool: tool.to_string(),
            target_path: path.map(str::to_string),
            error_snippet: snippet.to_string(),
        }
    }

    /// Recorder preserves submission order (the goal-runtime body
    /// renders only the first ~3 entries, so order matters when more
    /// than 3 land in one turn).
    #[test]
    fn record_failed_write_appends_in_order() {
        let mut diff = TurnDiff::default();
        diff.record_failed_write(mk_attempt("edit_file", Some("a.rs"), "needle miss a"));
        diff.record_failed_write(mk_attempt("write_file", Some("b.rs"), "path not found b"));
        diff.record_failed_write(mk_attempt(
            "delete_file",
            Some("c.rs"),
            "permission denied c",
        ));
        let recorded = diff.failed_write_attempts();
        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0].tool, "edit_file");
        assert_eq!(recorded[1].tool, "write_file");
        assert_eq!(recorded[2].tool, "delete_file");
        assert_eq!(recorded[0].target_path.as_deref(), Some("a.rs"));
        assert_eq!(recorded[2].error_snippet, "permission denied c");
        // A failed-only turn must still report `is_empty() == true`
        // — that is precisely the doom-loop telemetry gap the
        // tri-state classifier in `goal_runtime` plugs.
        assert!(
            diff.is_empty(),
            "is_empty() must reflect ONLY successful writes; failed attempts are a separate channel"
        );
    }

    /// `reset()` must clear both `writes` AND `failed_write_attempts`
    /// so the per-iteration scoping invariant continues to hold for
    /// the new channel. A bug here would leak the previous turn's
    /// rejections into the next continuation body.
    #[test]
    fn reset_clears_failed_attempts() {
        let mut diff = TurnDiff::default();
        diff.record_modify(PathBuf::from("a.rs"), 5);
        diff.record_failed_write(mk_attempt("edit_file", Some("a.rs"), "needle miss"));
        diff.record_failed_write(mk_attempt("write_file", None, "<unparseable>"));
        assert!(!diff.is_empty());
        assert_eq!(diff.failed_write_attempts().len(), 2);
        diff.reset();
        assert!(diff.is_empty());
        assert!(
            diff.failed_write_attempts().is_empty(),
            "reset() must wipe the failed-attempts channel alongside the writes map"
        );
    }
}
