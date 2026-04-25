//! Per-task aggregate derived from a [`TaskExecutionResult`], plus the
//! related commit / chunk-guard markers.
//!
//! Lives in its own module so the dispatch root (`mod.rs`) only has to
//! `pub(crate) use` the items the rest of `dev_loop` (and `task_run.rs`)
//! consume. The aggregation logic itself is intentionally side-effect
//! free so it can be unit-tested without touching the runner.

use std::collections::{HashMap, HashSet};

use aura_agent::agent_runner::TaskExecutionResult;
use aura_reasoner::{ContentBlock, Role, ToolResultContent};

/// Reason string attached to `AutomatonEvent::CommitSkipped` when
/// the DoD precheck trips. Kept as a module constant so the tick.rs
/// skip path and any future task-run callers stay in lockstep.
pub(crate) const COMMIT_SKIPPED_NO_CHANGES: &str =
    "no file changes or verification evidence; skipping commit to avoid orphan commits";

/// Marker the pre-dispatch chunk guard stamps onto every synthetic
/// `tool_result` when a `write_file` is short-circuited for exceeding
/// [`aura_agent::constants::WRITE_FILE_CHUNK_BYTES`]. Scanning for
/// this marker in `TaskAggregate::from_exec` is how the safety net
/// recovers the set of paths the agent was told to fall back to
/// chunked `edit_file` appends on — see the docstring on
/// [`TaskAggregate::pending_oversized_writes`] for why this is
/// promoted from an opaque log marker into a gate-blocking signal.
pub(crate) const CHUNK_GUARD_MARKER: &str = "[CHUNK_GUARD]";

/// Per-task aggregate the automaton consults before dispatching
/// `git_commit` / `git_commit_push`. The canonical server-side
/// `CachedTaskOutput` lives in `aura-os-server` and is not visible
/// from this crate, so we derive an equivalent shape locally from
/// the `TaskExecutionResult` the runner hands back:
///
/// * `files_changed` counts the file-mutation evidence we can see
///   without talking to the server: `exec.file_ops` (the canonical
///   list of writes the runner actually applied) plus the set of
///   unique `path`s from successful `write_file` / `edit_file` /
///   `delete_file` `tool_result`s in the task's message log. Using
///   the max of the two is a belt-and-braces signal: `file_ops`
///   may lag when the runner abandoned a partially-applied batch,
///   and the message scan catches successful tool calls whose side
///   effects never made it into `file_ops`.
///
/// * `verification_steps` counts successful `run_command`
///   `tool_result`s. `run_command` is the only generic shell tool
///   in the catalog (see `aura_agent::constants::COMMAND_TOOLS`)
///   so build / test / fmt / lint invocations all flow through it;
///   a single non-error result is treated as verification evidence.
///
/// `should_skip_commit` trips only when both counters are zero,
/// matching the plan's "no file changes AND no verification
/// evidence" trigger for the commit-skip DoD precheck.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskAggregate {
    pub files_changed: usize,
    pub verification_steps: usize,
    /// Paths whose initial `write_file` was rejected by the
    /// pre-dispatch chunk guard (see `aura-agent`'s
    /// `partition_oversized_writes`) and for which no subsequent
    /// successful write has been observed for the SAME path.
    ///
    /// Populated by scanning `exec.messages` for `tool_result` blocks
    /// whose content starts with the [`CHUNK_GUARD_MARKER`], resolving
    /// the originating `tool_use`'s `path` input, and then checking
    /// whether a later non-error `write_file`/`edit_file` tool_result
    /// for that same path appears in the log.
    ///
    /// A non-empty list means the agent was told "write a small stub
    /// now and fill the rest with `edit_file` chunks" but ran out of
    /// turns / context before finishing. Treating `task_completed`
    /// as success in that state is the exact bug that left
    /// `zero-sdk/src/messaging/group/types.rs` at ~2 KB of an
    /// 8 KB intended payload; the safety net trips on this list and
    /// routes the task to `record_task_failure` instead.
    pub pending_oversized_writes: Vec<String>,
}

impl TaskAggregate {
    /// Derive the aggregate from a completed `TaskExecutionResult`.
    pub(crate) fn from_exec(exec: &TaskExecutionResult) -> Self {
        let mut tool_uses: HashMap<String, String> = HashMap::new();
        let mut tool_use_paths: HashMap<String, String> = HashMap::new();
        let mut successful_file_paths: HashSet<String> = HashSet::new();
        let mut chunk_guarded_paths: HashSet<String> = HashSet::new();
        let mut verification_steps: usize = 0;

        // First pass: index tool_use ids -> tool name so we can
        // classify the matching tool_result blocks in the second
        // pass. The assistant always emits the tool_use before the
        // user-side tool_result, so a single forward pass over the
        // message log is sufficient.
        for msg in &exec.messages {
            match msg.role {
                Role::Assistant => {
                    for block in &msg.content {
                        if let ContentBlock::ToolUse { id, name, input } = block {
                            tool_uses.insert(id.clone(), name.clone());
                            if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
                                tool_use_paths.insert(id.clone(), path.to_string());
                            }
                        }
                    }
                }
                Role::User => {
                    for block in &msg.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            is_error,
                            content,
                            ..
                        } = block
                        {
                            // Chunk-guard safety net: every short-
                            // circuited oversized `write_file` fires
                            // an `is_error=true` tool_result whose
                            // content is prefixed with
                            // `[CHUNK_GUARD]`. Record the target path
                            // so the post-scan below can check
                            // whether the agent actually followed up
                            // with a successful write for the same
                            // file.
                            if *is_error && content_starts_with_marker(content, CHUNK_GUARD_MARKER)
                            {
                                if let Some(path) = tool_use_paths.get(tool_use_id) {
                                    chunk_guarded_paths.insert(path.clone());
                                }
                            }
                            if *is_error {
                                continue;
                            }
                            let Some(tool_name) = tool_uses.get(tool_use_id) else {
                                continue;
                            };
                            match tool_name.as_str() {
                                "write_file" | "edit_file" | "delete_file" => {
                                    // Recover the path argument from
                                    // the originating tool_use to
                                    // dedupe repeated writes to the
                                    // same file. Tool_uses without a
                                    // `path` field still count via
                                    // their id (rare: malformed
                                    // input).
                                    let path_key = exec
                                        .messages
                                        .iter()
                                        .flat_map(|m| m.content.iter())
                                        .find_map(|b| match b {
                                            ContentBlock::ToolUse { id, input, .. }
                                                if id == tool_use_id =>
                                            {
                                                input
                                                    .get("path")
                                                    .and_then(|v| v.as_str())
                                                    .map(str::to_string)
                                            }
                                            _ => None,
                                        })
                                        .unwrap_or_else(|| format!("<id:{tool_use_id}>"));
                                    successful_file_paths.insert(path_key);
                                }
                                "run_command" => {
                                    verification_steps += 1;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        // Prefer the max of the runner-reported `file_ops` count and
        // the message-derived count (see struct docs).
        let files_changed = exec.file_ops.len().max(successful_file_paths.len());

        // Resolve the chunk-guard safety net: any CHUNK_GUARDed path
        // that DOES have a successful write later in the log is
        // considered recovered. The agent may have followed the
        // recovery hint ("write stub + edit_file appends") to
        // completion, in which case we must NOT block the task.
        // Anything still unresolved pins the task into the pending-
        // oversized-writes failure path in `record_task_success`.
        let mut pending_oversized_writes: Vec<String> = chunk_guarded_paths
            .into_iter()
            .filter(|path| !successful_file_paths.contains(path))
            .collect();
        pending_oversized_writes.sort();

        Self {
            files_changed,
            verification_steps,
            pending_oversized_writes,
        }
    }

    /// DoD precheck: skip the commit only when we have neither any
    /// observed file changes nor any verification-step evidence.
    pub(crate) fn should_skip_commit(&self) -> bool {
        self.files_changed == 0 && self.verification_steps == 0
    }

    /// Chunk-guard safety net: the agent was short-circuited on at
    /// least one oversized `write_file` (see the pre-dispatch
    /// `partition_oversized_writes` in `aura-agent`) and never
    /// followed up with a successful write for that path. Treating
    /// the task as "done" in this state is how the original
    /// task_id=4079e975 regression left
    /// `zero-sdk/src/messaging/group/types.rs` at ~2 KB of an
    /// 8 KB intended payload. Callers (see `tick::record_task_success`)
    /// route the task to `record_task_failure` instead so the retry
    /// ladder gets another shot at finishing the chunked writes.
    pub(crate) fn has_pending_oversized_writes(&self) -> bool {
        !self.pending_oversized_writes.is_empty()
    }
}

fn content_starts_with_marker(content: &ToolResultContent, marker: &str) -> bool {
    match content {
        ToolResultContent::Text(t) => t.trim_start().starts_with(marker),
        // The chunk guard only ever stamps plain-text marker strings
        // (see `partition_oversized_writes`), so a JSON payload can't
        // be a chunk-guard result by construction.
        ToolResultContent::Json(_) => false,
    }
}
