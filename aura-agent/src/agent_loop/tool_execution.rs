//! Tool result processing, caching, and build checks.

use std::collections::HashSet;

use aura_reasoner::{ContentBlock, Message, ModelResponse, ToolResultContent};
use tokio::sync::mpsc::UnboundedSender;
use tracing::warn;

use crate::blocking::detection::{detect_all_blocked, BlockingContext};
use crate::blocking::stall::StallDetector;
use crate::budget::ExplorationState;
use crate::build;
use crate::events::AgentLoopEvent;
use crate::helpers;
use crate::read_guard::ReadGuardState;
use crate::types::{AgentToolExecutor, BuildBaseline, ToolCallInfo, ToolCallResult};

use super::streaming;
use super::{AgentLoop, AgentLoopConfig, LoopState};

/// Tools whose successful results can be cached within a single agent run.
const CACHEABLE_TOOLS: &[&str] = &["fs_read", "fs_ls", "fs_stat", "fs_find", "search_code"];

fn is_cacheable(tool_name: &str) -> bool {
    CACHEABLE_TOOLS.contains(&tool_name)
}

fn cache_key(tool_name: &str, input: &serde_json::Value) -> String {
    let canonical = serde_json::to_string(input).unwrap_or_default();
    format!("{tool_name}\0{canonical}")
}

// ---------------------------------------------------------------------------
// Top-level ToolUse stop-reason handler
// ---------------------------------------------------------------------------

/// Handle `StopReason::ToolUse` — cache, execute, emit, stall-check.
///
/// Returns `true` if the loop should break.
pub(super) async fn handle_tool_use(
    agent: &AgentLoop,
    response: &ModelResponse,
    executor: &dyn AgentToolExecutor,
    event_tx: &Option<UnboundedSender<AgentLoopEvent>>,
    state: &mut LoopState,
) -> bool {
    let tool_calls = extract_tool_calls(response);
    if tool_calls.is_empty() {
        return true;
    }

    let (cached_results, uncached_calls) = split_cached(&tool_calls, &state.tool_cache);

    let (executed_results, is_stalled) = if uncached_calls.is_empty() {
        (Vec::new(), false)
    } else {
        agent
            .process_tool_results(&uncached_calls, executor, state)
            .await
    };

    update_cache(&mut state.tool_cache, &uncached_calls, &executed_results);

    let mut all_results: Vec<ToolCallResult> = cached_results;
    all_results.extend(executed_results);
    emit_tool_results(event_tx, &all_results, &tool_calls);

    let should_stop = all_results.iter().any(|r| r.stop_loop);
    push_tool_result_message(&mut state.messages, all_results);

    if should_stop {
        return true;
    }

    if is_stalled {
        handle_stall(event_tx, state);
        return true;
    }

    false
}

// ---------------------------------------------------------------------------
// Tool call extraction and caching
// ---------------------------------------------------------------------------

fn extract_tool_calls(response: &ModelResponse) -> Vec<ToolCallInfo> {
    response
        .message
        .content
        .iter()
        .filter_map(|block| {
            if let ContentBlock::ToolUse { id, name, input } = block {
                Some(ToolCallInfo {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                })
            } else {
                None
            }
        })
        .collect()
}

fn split_cached(
    tool_calls: &[ToolCallInfo],
    cache: &std::collections::HashMap<String, String>,
) -> (Vec<ToolCallResult>, Vec<ToolCallInfo>) {
    let mut cached = Vec::new();
    let mut uncached = Vec::new();

    for tc in tool_calls {
        if is_cacheable(&tc.name) {
            let key = cache_key(&tc.name, &tc.input);
            if let Some(hit) = cache.get(&key) {
                cached.push(ToolCallResult {
                    tool_use_id: tc.id.clone(),
                    content: hit.clone(),
                    is_error: false,
                    stop_loop: false,
                });
                continue;
            }
        }
        uncached.push(tc.clone());
    }

    (cached, uncached)
}

fn update_cache(
    cache: &mut std::collections::HashMap<String, String>,
    uncached: &[ToolCallInfo],
    executed: &[ToolCallResult],
) {
    let any_write = uncached.iter().any(|tc| {
        helpers::is_write_tool(&tc.name)
            && executed
                .iter()
                .any(|r| r.tool_use_id == tc.id && !r.is_error)
    });
    if any_write {
        cache.clear();
    }

    for r in executed {
        if let Some(tc) = uncached.iter().find(|t| t.id == r.tool_use_id) {
            if is_cacheable(&tc.name) && !r.is_error {
                let key = cache_key(&tc.name, &tc.input);
                cache.insert(key, r.content.clone());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Event emission and message helpers
// ---------------------------------------------------------------------------

fn emit_tool_results(
    event_tx: &Option<UnboundedSender<AgentLoopEvent>>,
    all_results: &[ToolCallResult],
    tool_calls: &[ToolCallInfo],
) {
    for r in all_results {
        let tool_name = tool_calls
            .iter()
            .find(|t| t.id == r.tool_use_id)
            .map_or_else(String::new, |t| t.name.clone());
        streaming::emit(
            event_tx,
            AgentLoopEvent::ToolResult {
                tool_use_id: r.tool_use_id.clone(),
                tool_name,
                content: r.content.clone(),
                is_error: r.is_error,
            },
        );
    }
}

fn push_tool_result_message(messages: &mut Vec<Message>, results: Vec<ToolCallResult>) {
    let converted: Vec<(String, ToolResultContent, bool)> = results
        .into_iter()
        .map(|r| (r.tool_use_id, ToolResultContent::text(r.content), r.is_error))
        .collect();
    if !converted.is_empty() {
        messages.push(Message::tool_results(converted));
    }
}

fn handle_stall(event_tx: &Option<UnboundedSender<AgentLoopEvent>>, state: &mut LoopState) {
    let msg = "CRITICAL: Agent appears stalled — repeatedly failing \
               to write to the same files. Stopping to prevent \
               infinite loop. Try a different approach or ask for help.";
    helpers::push_or_replace_warning(&mut state.messages, msg);
    streaming::emit(
        event_tx,
        AgentLoopEvent::Error {
            code: "stall_detected".to_string(),
            message: msg.to_string(),
            recoverable: false,
        },
    );
    state.result.stalled = true;
}

// ---------------------------------------------------------------------------
// Core tool result processing (blocking, execution, tracking, build)
// ---------------------------------------------------------------------------

impl AgentLoop {
    /// Process tool call results from one iteration.
    ///
    /// Applies blocking detection, tracks writes/reads/commands,
    /// manages exploration budget, and handles build checks.
    pub(crate) async fn process_tool_results(
        &self,
        tool_calls: &[ToolCallInfo],
        executor: &dyn AgentToolExecutor,
        state: &mut LoopState,
    ) -> (Vec<ToolCallResult>, bool) {
        let (blocked_results, to_execute) = partition_blocked(
            tool_calls,
            &mut state.blocking_ctx,
            &mut state.read_guard,
            &mut state.messages,
        );

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
            run_auto_build(
                &self.config,
                executor,
                &mut state.messages,
                &mut state.build_cooldown,
                state.build_baseline.as_ref(),
            )
            .await;
        }

        if any_write_success {
            state.blocking_ctx.exploration_allowance += 2;
        }

        let mut all_results = blocked_results;
        all_results.extend(executed);
        (all_results, stalled)
    }
}

fn partition_blocked(
    tool_calls: &[ToolCallInfo],
    blocking_ctx: &mut BlockingContext,
    read_guard: &mut ReadGuardState,
    messages: &mut Vec<Message>,
) -> (Vec<ToolCallResult>, Vec<ToolCallInfo>) {
    let mut blocked = Vec::new();
    let mut to_execute = Vec::new();

    for tool in tool_calls {
        let check = detect_all_blocked(tool, blocking_ctx, read_guard);
        if check.blocked {
            let msg = check
                .recovery_message
                .unwrap_or_else(|| "Blocked".to_string());
            helpers::push_or_replace_warning(messages, &msg);
            blocked.push(ToolCallResult {
                tool_use_id: tool.id.clone(),
                content: msg,
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

    for exec_result in executed {
        if let Some(tool) = to_execute.iter().find(|t| t.id == exec_result.tool_use_id) {
            if helpers::is_write_tool(&tool.name) {
                if let Some(path) = tool.input.get("path").and_then(|v| v.as_str()) {
                    write_targets.insert(path.to_string());
                    if !exec_result.is_error {
                        any_write_success = true;
                    }
                }
            }
        }
    }

    let stalled = stall_detector.update(&write_targets, any_write_success);
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
    messages: &mut Vec<Message>,
    build_cooldown: &mut usize,
    build_baseline: Option<&BuildBaseline>,
) {
    if let Some(build_result) = executor.auto_build_check().await {
        *build_cooldown = config.auto_build_cooldown;
        if !build_result.success {
            let annotated = build_baseline.map_or_else(
                || build_result.output.clone(),
                |baseline| build::annotate_build_output(&build_result.output, baseline),
            );
            messages.push(Message::user(format!(
                "Build check failed with {} error(s):\n\n{annotated}",
                build_result.error_count
            )));
        }
    }
}
