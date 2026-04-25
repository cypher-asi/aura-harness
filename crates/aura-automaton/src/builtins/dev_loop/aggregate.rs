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

/// Mutable accumulator threaded through the per-block fold helpers.
/// Bundling these into one struct keeps the helper signatures from
/// growing unwieldy and makes the data-flow explicit: every helper
/// reads the indices populated by the assistant fold and contributes
/// to one or more of the result counters / sets.
#[derive(Default)]
struct FoldState {
    /// `tool_use.id` -> `tool_use.name`. Populated by
    /// [`fold_tool_use`] so [`fold_tool_result`] can classify a
    /// `tool_result` without re-scanning the assistant messages.
    tool_uses: HashMap<String, String>,
    /// `tool_use.id` -> `tool_use.input.path`. Populated only for
    /// tool_uses whose `input.path` is a string; consumed by the
    /// chunk-guard branch in [`fold_tool_result`] to attribute a
    /// short-circuited write to the originating path.
    tool_use_paths: HashMap<String, String>,
    /// Unique `path` keys for which a non-error
    /// `write_file`/`edit_file`/`delete_file` tool_result was seen.
    successful_file_paths: HashSet<String>,
    /// Paths whose `write_file` was rejected by the chunk guard.
    /// Compared against `successful_file_paths` in
    /// [`resolve_pending_oversized_writes`] to decide which paths
    /// the agent never recovered.
    chunk_guarded_paths: HashSet<String>,
    /// Successful `run_command` tool_results — the
    /// `aura_agent::constants::COMMAND_TOOLS` allowlist routes every
    /// build/test/fmt/lint invocation through this single tool.
    verification_steps: usize,
}

impl TaskAggregate {
    /// Derive the aggregate from a completed `TaskExecutionResult`.
    ///
    /// Walks `exec.messages` once, dispatching each block to one of
    /// the per-block fold helpers below:
    ///
    /// * [`fold_tool_use`] populates the id->name / id->path indices
    ///   from `Role::Assistant` blocks.
    /// * [`fold_tool_result`] classifies `Role::User` `tool_result`
    ///   blocks: chunk-guard markers, file-write side effects, and
    ///   `run_command` verification evidence.
    ///
    /// After the walk, [`resolve_pending_oversized_writes`] computes
    /// the final pending-oversized-writes list. The assistant always
    /// emits the `tool_use` before the user-side `tool_result`, so a
    /// single forward pass over the message log is sufficient.
    pub(crate) fn from_exec(exec: &TaskExecutionResult) -> Self {
        let mut state = FoldState::default();

        for msg in &exec.messages {
            match msg.role {
                Role::Assistant => {
                    for block in &msg.content {
                        fold_tool_use(&mut state, block);
                    }
                }
                Role::User => {
                    for block in &msg.content {
                        fold_tool_result(&mut state, block, exec);
                    }
                }
            }
        }

        // Prefer the max of the runner-reported `file_ops` count and
        // the message-derived count (see struct docs).
        let files_changed = exec.file_ops.len().max(state.successful_file_paths.len());
        let pending_oversized_writes = resolve_pending_oversized_writes(
            state.chunk_guarded_paths,
            &state.successful_file_paths,
        );

        Self {
            files_changed,
            verification_steps: state.verification_steps,
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

/// Process a single block from a `Role::Assistant` message.
///
/// We only care about `ToolUse` blocks: every other variant (text,
/// thinking, …) is a no-op for aggregation purposes. The id->name
/// map drives the result classification in [`fold_tool_result`];
/// the id->path map is consumed by the chunk-guard branch.
fn fold_tool_use(state: &mut FoldState, block: &ContentBlock) {
    if let ContentBlock::ToolUse { id, name, input } = block {
        state.tool_uses.insert(id.clone(), name.clone());
        if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
            state.tool_use_paths.insert(id.clone(), path.to_string());
        }
    }
}

/// Process a single block from a `Role::User` message.
///
/// Only `ToolResult` blocks are interesting; the function early-returns
/// for any other variant. Behaviour mirrors the original inline body
/// exactly, including the chunk-guard marker scan that runs even when
/// `is_error == true`.
fn fold_tool_result(state: &mut FoldState, block: &ContentBlock, exec: &TaskExecutionResult) {
    let ContentBlock::ToolResult {
        tool_use_id,
        is_error,
        content,
        ..
    } = block
    else {
        return;
    };

    // Chunk-guard safety net: every short-circuited oversized
    // `write_file` fires an `is_error=true` tool_result whose content
    // is prefixed with `[CHUNK_GUARD]`. Record the target path so the
    // post-scan below can check whether the agent actually followed up
    // with a successful write for the same file.
    if *is_error && content_starts_with_marker(content, CHUNK_GUARD_MARKER) {
        if let Some(path) = state.tool_use_paths.get(tool_use_id) {
            state.chunk_guarded_paths.insert(path.clone());
        }
    }
    if *is_error {
        return;
    }
    let Some(tool_name) = state.tool_uses.get(tool_use_id) else {
        return;
    };
    match tool_name.as_str() {
        "write_file" | "edit_file" | "delete_file" => {
            state
                .successful_file_paths
                .insert(resolve_tool_use_path(exec, tool_use_id));
        }
        "run_command" => {
            state.verification_steps += 1;
        }
        _ => {}
    }
}

/// Recover the `path` argument from the originating `tool_use` to
/// dedupe repeated writes to the same file. `tool_use`s without a
/// `path` field still count via their id (rare: malformed input).
///
/// Kept as a free function rather than reading from
/// [`FoldState::tool_use_paths`] so the lookup logic stays
/// byte-identical to the pre-refactor inline scan; the original
/// fallback was `<id:{tool_use_id}>` even when `tool_use_paths` had
/// already skipped the entry due to a non-string `path`. Going through
/// the message log preserves that fallback behaviour.
fn resolve_tool_use_path(exec: &TaskExecutionResult, tool_use_id: &str) -> String {
    exec.messages
        .iter()
        .flat_map(|m| m.content.iter())
        .find_map(|b| match b {
            ContentBlock::ToolUse { id, input, .. } if id == tool_use_id => input
                .get("path")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            _ => None,
        })
        .unwrap_or_else(|| format!("<id:{tool_use_id}>"))
}

/// Resolve the chunk-guard safety net: any `CHUNK_GUARD`ed path that
/// DOES have a successful write later in the log is considered
/// recovered. The agent may have followed the recovery hint ("write
/// stub + edit_file appends") to completion, in which case we must
/// NOT block the task. Anything still unresolved pins the task into
/// the pending-oversized-writes failure path in
/// `record_task_success`.
///
/// Result is sorted so callers / tests get a deterministic ordering.
fn resolve_pending_oversized_writes(
    chunk_guarded_paths: HashSet<String>,
    successful_file_paths: &HashSet<String>,
) -> Vec<String> {
    let mut pending: Vec<String> = chunk_guarded_paths
        .into_iter()
        .filter(|path| !successful_file_paths.contains(path))
        .collect();
    pending.sort();
    pending
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
