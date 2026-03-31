//! Core tool result processing: blocking, execution, effect tracking, build.

use std::collections::HashSet;

use crate::blocking::detection::{detect_all_blocked, BlockingContext};
use crate::blocking::stall::StallDetector;
use crate::budget::ExplorationState;
use crate::build;
use crate::helpers;
use crate::read_guard::ReadGuardState;
use crate::types::{AgentToolExecutor, BuildBaseline, ToolCallInfo, ToolCallResult};
use tracing::warn;

use super::{AgentLoop, AgentLoopConfig, LoopState};

impl AgentLoop {
    /// Process tool call results from one iteration.
    ///
    /// Returns `(results, side_messages, is_stalled, blocked_ids)` where
    /// `side_messages` are warning/build texts that should be embedded into
    /// the `tool_result` user message rather than pushed as separate messages
    /// (which would violate Anthropic's `tool_use/tool_result` adjacency
    /// requirement), and `blocked_ids` tracks which tool calls were blocked
    /// by detection policy (for accurate source labelling in logs).
    pub(crate) async fn process_tool_results(
        &self,
        tool_calls: &[ToolCallInfo],
        executor: &dyn AgentToolExecutor,
        state: &mut LoopState,
    ) -> (Vec<ToolCallResult>, Vec<String>, bool, HashSet<String>) {
        let mut side_messages: Vec<String> = Vec::new();

        let (blocked_results, to_execute) = partition_blocked(
            tool_calls,
            &state.blocking_ctx,
            &state.read_guard,
            &mut side_messages,
        );

        let blocked_ids: HashSet<String> = blocked_results
            .iter()
            .map(|r| r.tool_use_id.clone())
            .collect();

        let executed = if to_execute.is_empty() {
            Vec::new()
        } else {
            executor.execute(&to_execute).await
        };

        let any_write_success = track_tool_effects(
            &to_execute,
            &executed,
            &mut state.blocking_ctx,
            &mut state.read_guard,
            &mut state.exploration_state,
            &mut state.had_any_write,
        );

        let stalled = check_stall_detection(&mut state.stall_detector, &to_execute, &executed);

        if any_write_success && state.build_cooldown == 0 {
            if let Some(build_text) = run_auto_build(
                &self.config,
                executor,
                &mut state.build_cooldown,
                state.build_baseline.as_ref(),
            )
            .await
            {
                side_messages.push(build_text);
            }
        }

        if any_write_success {
            state.blocking_ctx.exploration_allowance += 2;
        }

        let mut all_results = blocked_results;
        all_results.extend(executed);
        (all_results, side_messages, stalled, blocked_ids)
    }
}

fn partition_blocked(
    tool_calls: &[ToolCallInfo],
    blocking_ctx: &BlockingContext,
    read_guard: &ReadGuardState,
    side_messages: &mut Vec<String>,
) -> (Vec<ToolCallResult>, Vec<ToolCallInfo>) {
    let mut blocked = Vec::new();
    let mut to_execute = Vec::new();

    for tool in tool_calls {
        let check = detect_all_blocked(tool, blocking_ctx, read_guard);
        if check.blocked {
            let msg = check
                .recovery_message
                .unwrap_or_else(|| "Blocked".to_string());
            let path_hint = tool
                .input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            warn!(
                tool_use_id = %tool.id,
                tool_name = %tool.name,
                path = path_hint,
                reason = %msg,
                "Tool call blocked by detection policy"
            );
            side_messages.push(msg.clone());
            blocked.push(ToolCallResult {
                tool_use_id: tool.id.clone(),
                content: format!("[BLOCKED] {msg}"),
                is_error: true,
                stop_loop: false,
            });
        } else {
            to_execute.push(tool.clone());
        }
    }

    (blocked, to_execute)
}

fn track_tool_effects(
    to_execute: &[ToolCallInfo],
    executed: &[ToolCallResult],
    blocking_ctx: &mut BlockingContext,
    read_guard: &mut ReadGuardState,
    exploration_state: &mut ExplorationState,
    had_any_write: &mut bool,
) -> bool {
    let mut any_write_success = false;

    for exec_result in executed {
        let Some(tool) = to_execute.iter().find(|t| t.id == exec_result.tool_use_id) else {
            continue;
        };

        if helpers::is_exploration_tool(&tool.name) {
            exploration_state.count += 1;
            if let Some(path) = tool.input.get("path").and_then(|v| v.as_str()) {
                if tool.input.get("start_line").is_some() {
                    read_guard.record_range_read(path);
                } else {
                    read_guard.record_full_read(path);
                }
            }
        }

        if helpers::is_write_tool(&tool.name) {
            if let Some(path) = tool.input.get("path").and_then(|v| v.as_str()) {
                if exec_result.is_error {
                    blocking_ctx.on_write_failure(path);
                } else {
                    blocking_ctx.on_write_success(path, read_guard);
                    any_write_success = true;
                    *had_any_write = true;
                }
            } else if exec_result.is_error {
                blocking_ctx.on_malformed_write();
            }
        }

        if crate::constants::COMMAND_TOOLS.contains(&tool.name.as_str()) {
            blocking_ctx.on_command_result(!exec_result.is_error);
        }
    }

    any_write_success
}

fn check_stall_detection(
    stall_detector: &mut StallDetector,
    to_execute: &[ToolCallInfo],
    executed: &[ToolCallResult],
) -> bool {
    let mut write_targets = HashSet::new();
    let mut any_write_success = false;
    let mut writes_attempted = false;

    for exec_result in executed {
        if let Some(tool) = to_execute.iter().find(|t| t.id == exec_result.tool_use_id) {
            if helpers::is_write_tool(&tool.name) {
                writes_attempted = true;
                if let Some(path) = tool.input.get("path").and_then(|v| v.as_str()) {
                    write_targets.insert(path.to_string());
                    if !exec_result.is_error {
                        any_write_success = true;
                    }
                }
            }
        }
    }

    let stalled = stall_detector.update(&write_targets, any_write_success, writes_attempted);
    if stalled {
        warn!(
            streak = stall_detector.streak(),
            "Stall detected: same write targets failing repeatedly"
        );
    }
    stalled
}

async fn run_auto_build(
    config: &AgentLoopConfig,
    executor: &dyn AgentToolExecutor,
    build_cooldown: &mut usize,
    build_baseline: Option<&BuildBaseline>,
) -> Option<String> {
    if let Some(build_result) = executor.auto_build_check().await {
        *build_cooldown = config.auto_build_cooldown;
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
