//! Cumulative per-batch tool effects (steering, read/write tracking,
//! file-change journal) and the post-write auto-build check.

use std::path::PathBuf;

use crate::build;
use crate::helpers;
use crate::types::{
    AgentToolExecutor, BuildBaseline, FileChange, FileChangeKind, ToolCallInfo, ToolCallResult,
};

use super::super::turn_diff::TurnDiff;
use super::super::{AgentLoopConfig, LoopState};

// Phase 8: the previously-threaded seven `&mut` field borrows
// (`result`, `exploration_state`, `had_any_write`, `turn_diff`,
// `steering`, `session_read_paths`, `read_after_write_allowances`)
// collapse into a single `state: &mut LoopState` borrow. Every
// borrow this function takes is a disjoint field on `LoopState`, so
// the field-projection pattern keeps the borrow checker happy
// without sacrificing the "this is the per-batch effect surface"
// boundary. Phase 5 already collapsed the steering trackers into
// `state.steering`; Phase 8 finishes the consolidation by removing
// the last six explicit `&mut` field arguments at the call site.
pub(super) fn track_tool_effects(
    to_execute: &[ToolCallInfo],
    executed: &[ToolCallResult],
    state: &mut LoopState,
) -> bool {
    let mut any_write_success = false;

    for exec_result in executed {
        let Some(tool) = to_execute.iter().find(|t| t.id == exec_result.tool_use_id) else {
            continue;
        };

        // Phase 5: fan `(tool, exec_result)` out to every installed
        // steering source. Each source decides for itself whether
        // the pair is interesting (`RepeatedReadTracker` only cares
        // about successful `read_file`; `ImplementNowSteering`
        // tracks exploration/write deltas; `EarlyTestOracle` drives
        // its state machine on tool names). The buffered and pump
        // transports both reach `track_tool_effects` so this single
        // call site keeps the registry behaviour identical across
        // transports — the contract pinned by `transport_parity`.
        state.steering.observe_tool(tool, exec_result);

        if helpers::is_exploration_tool(&tool.name) {
            track_exploration(tool, exec_result, state);
        }

        if helpers::is_write_tool(&tool.name) && track_write(tool, exec_result, state) {
            any_write_success = true;
        }
    }

    any_write_success
}

fn record_into_turn_diff(turn_diff: &mut TurnDiff, change: &FileChange) {
    let path = std::path::PathBuf::from(&change.path);
    match change.kind {
        FileChangeKind::Create => turn_diff.record_create(path),
        FileChangeKind::Modify => {
            // No raw byte count is plumbed through `FileChange`
            // today; the codex tracker uses an approximation too.
            // Use `lines_added` as a cheap proxy so the count is
            // monotonic and non-zero for typical edits.
            let bytes = change.lines_added as usize;
            turn_diff.record_modify(path, bytes);
        }
        FileChangeKind::Delete => turn_diff.record_delete(path),
    }
}

fn track_exploration(tool: &ToolCallInfo, exec_result: &ToolCallResult, state: &mut LoopState) {
    state.exploration_state.count += 1;
    if exec_result.is_error {
        return;
    }
    let Some(path) = tool.input.get("path").and_then(|v| v.as_str()) else {
        return;
    };
    let path_buf = PathBuf::from(path);
    state.turn_diff.record_read(path_buf.clone());
    state.session_read_paths.insert(path_buf);
    let key = PathBuf::from(path);
    let mut exhausted = false;
    if let Some(remaining) = state.read_after_write_allowances.get_mut(&key) {
        *remaining = remaining.saturating_sub(1);
        exhausted = *remaining == 0;
    }
    if exhausted {
        state.read_after_write_allowances.remove(&key);
    }
}

/// Returns `true` when the write landed successfully.
fn track_write(tool: &ToolCallInfo, exec_result: &ToolCallResult, state: &mut LoopState) -> bool {
    // Skip failed write attempts entirely: the tool layer
    // already returned an error to the model, and the
    // codex-parity loop no longer harvests per-iteration
    // failed-write telemetry for continuation injection.
    if exec_result.is_error {
        return false;
    }

    let path_arg = tool.input.get("path").and_then(|v| v.as_str());
    if let Some(path) = path_arg {
        let path_buf = PathBuf::from(path);
        state.session_read_paths.remove(&path_buf);
        state.read_after_write_allowances.insert(
            path_buf,
            aura_config::agent().tools.reads_after_write_allowance,
        );
        state.had_any_write = true;
        for change in &exec_result.file_changes {
            state.result.record_file_change(change.clone());
            record_into_turn_diff(&mut state.turn_diff, change);
        }
        true
    } else if !exec_result.file_changes.is_empty() {
        // Multi-file write fallback: the tool has no single
        // `path` argument but the result carries one
        // `FileChange` per touched file. Each change is
        // treated as a successful write so the file-change
        // journal and Phase B's `had_any_file_write` latch
        // light up the same way they do for the granular
        // write tools.
        for change in &exec_result.file_changes {
            let path_buf = PathBuf::from(&change.path);
            state.session_read_paths.remove(&path_buf);
            state.read_after_write_allowances.insert(
                path_buf,
                aura_config::agent().tools.reads_after_write_allowance,
            );
            state.result.record_file_change(change.clone());
            record_into_turn_diff(&mut state.turn_diff, change);
        }
        state.had_any_write = true;
        true
    } else {
        false
    }
}

pub(super) async fn run_auto_build(
    config: &AgentLoopConfig,
    executor: &dyn AgentToolExecutor,
    build_cooldown: &mut usize,
    build_baseline: Option<&BuildBaseline>,
    changed_files: &[String],
    decisions: &mut Vec<aura_core_types::trace::HarnessDecision>,
) -> Option<String> {
    if let Some(build_result) = executor.auto_build_check().await {
        *build_cooldown = config.auto_build_cooldown;
        decisions.push(super::super::decisions::auto_build(
            build_result.success,
            build_result.error_count,
            changed_files,
        ));
        if !build_result.success {
            let annotated = build_baseline.map_or_else(
                || build_result.output.clone(),
                |baseline| build::annotate_build_output(&build_result.output, baseline),
            );
            return Some(format!(
                "Build check failed with {} error(s):\n\n{annotated}",
                build_result.error_count
            ));
        }
    }
    None
}
