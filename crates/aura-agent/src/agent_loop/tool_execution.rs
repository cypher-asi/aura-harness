//! Tool result processing, caching, and build checks.

use std::collections::HashSet;

use aura_core::{tool_result_cache_key, CACHEABLE_TOOLS};
use aura_reasoner::{ContentBlock, Message, ModelResponse, ToolResultContent};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{info, warn};

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

fn is_cacheable(tool_name: &str) -> bool {
    CACHEABLE_TOOLS.contains(&tool_name)
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
    event_tx: Option<&UnboundedSender<AgentLoopEvent>>,
    state: &mut LoopState,
) -> bool {
    let tool_calls = extract_tool_calls(response);
    if tool_calls.is_empty() {
        return true;
    }
    info!(
        tool_count = tool_calls.len(),
        "Processing tool_use stop reason"
    );
    for tc in &tool_calls {
        info!(
            tool_use_id = %tc.id,
            tool_name = %tc.name,
            is_write = helpers::is_write_tool(&tc.name),
            "Tool requested by model"
        );
    }

    let (cached_results, uncached_calls) = split_cached(&tool_calls, &state.tool_cache);
    let cached_ids: HashSet<String> = cached_results
        .iter()
        .map(|r| r.tool_use_id.clone())
        .collect();
    info!(
        cached_count = cached_results.len(),
        execute_count = uncached_calls.len(),
        "Resolved cached vs executable tool calls"
    );

    let (executed_results, side_messages, is_stalled, blocked_ids) = if uncached_calls.is_empty() {
        (Vec::new(), Vec::new(), false, HashSet::new())
    } else {
        agent
            .process_tool_results(&uncached_calls, executor, state)
            .await
    };

    update_cache(&mut state.tool_cache, &uncached_calls, &executed_results);

    let mut all_results: Vec<ToolCallResult> = cached_results;
    all_results.extend(executed_results);
    for r in &all_results {
        let tool_name = tool_calls
            .iter()
            .find(|t| t.id == r.tool_use_id)
            .map_or("unknown", |t| t.name.as_str());
        let source = if cached_ids.contains(&r.tool_use_id) {
            "cache"
        } else if blocked_ids.contains(&r.tool_use_id) {
            "blocked"
        } else {
            "executor"
        };
        info!(
            tool_use_id = %r.tool_use_id,
            tool_name = tool_name,
            is_write = helpers::is_write_tool(tool_name),
            is_error = r.is_error,
            stop_loop = r.stop_loop,
            source = source,
            result_len = r.content.len(),
            "Tool call completed"
        );
    }
    emit_tool_results(event_tx, &all_results, &tool_calls);

    let should_stop = all_results.iter().any(|r| r.stop_loop);

    let all_errors = !all_results.is_empty() && all_results.iter().all(|r| r.is_error);
    if all_errors {
        state.consecutive_all_error_iterations += 1;
    } else {
        state.consecutive_all_error_iterations = 0;
    }

    push_tool_result_message_with_context(&mut state.messages, all_results, side_messages);

    if should_stop {
        return true;
    }

    if is_stalled {
        handle_stall(event_tx, state);
        return true;
    }

    if state.consecutive_all_error_iterations
        >= crate::constants::CONSECUTIVE_ERROR_ITERATIONS_LIMIT
    {
        let msg = format!(
            "CRITICAL: All tool calls have returned errors for {} consecutive iterations. \
             The agent appears stuck. Stopping to prevent waste.",
            state.consecutive_all_error_iterations
        );
        helpers::append_warning(&mut state.messages, &msg);
        streaming::emit(
            event_tx,
            AgentLoopEvent::Error {
                code: "consecutive_errors".to_string(),
                message: msg,
                recoverable: false,
            },
        );
        state.result.stalled = true;
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
            let key = tool_result_cache_key(&tc.name, &tc.input);
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
                let key = tool_result_cache_key(&tc.name, &tc.input);
                cache.insert(key, r.content.clone());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Event emission and message helpers
// ---------------------------------------------------------------------------

fn emit_tool_results(
    event_tx: Option<&UnboundedSender<AgentLoopEvent>>,
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

/// Build a single user message with tool_result blocks first, followed by any
/// optional context text blocks.
///
/// Anthropic requires that every assistant `tool_use` is immediately paired by
/// `tool_result` blocks in the next user message. Keeping tool results first
/// avoids ambiguity from prepended warning/build text blocks.
fn push_tool_result_message_with_context(
    messages: &mut Vec<Message>,
    results: Vec<ToolCallResult>,
    context_texts: Vec<String>,
) {
    let mut blocks: Vec<ContentBlock> = Vec::new();
    for r in results {
        blocks.push(ContentBlock::tool_result(
            &r.tool_use_id,
            ToolResultContent::text(r.content),
            r.is_error,
        ));
    }
    for text in context_texts {
        blocks.push(ContentBlock::Text { text });
    }

    if !blocks.is_empty() {
        messages.push(Message::new(aura_reasoner::Role::User, blocks));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_results_are_emitted_before_context_texts() {
        let mut messages = Vec::new();
        let results = vec![
            ToolCallResult {
                tool_use_id: "tool_1".to_string(),
                content: "ok 1".to_string(),
                is_error: false,
                stop_loop: false,
            },
            ToolCallResult {
                tool_use_id: "tool_2".to_string(),
                content: "ok 2".to_string(),
                is_error: true,
                stop_loop: false,
            },
        ];
        let context = vec!["Build check failed".to_string()];

        push_tool_result_message_with_context(&mut messages, results, context);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, aura_reasoner::Role::User);
        assert!(matches!(
            messages[0].content.first(),
            Some(ContentBlock::ToolResult { tool_use_id, .. }) if tool_use_id == "tool_1"
        ));
        assert!(matches!(
            messages[0].content.get(1),
            Some(ContentBlock::ToolResult { tool_use_id, .. }) if tool_use_id == "tool_2"
        ));
        assert!(matches!(
            messages[0].content.get(2),
            Some(ContentBlock::Text { text }) if text == "Build check failed"
        ));
    }
}

fn handle_stall(event_tx: Option<&UnboundedSender<AgentLoopEvent>>, state: &mut LoopState) {
    let msg = "CRITICAL: Agent appears stalled — repeatedly failing \
               to write to the same files. Stopping to prevent \
               infinite loop. Try a different approach or ask for help.";
    helpers::append_warning(&mut state.messages, msg);
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
    /// Returns `(results, side_messages, is_stalled, blocked_ids)` where
    /// `side_messages` are warning/build texts that should be embedded into
    /// the tool_result user message rather than pushed as separate messages
    /// (which would violate Anthropic's tool_use/tool_result adjacency
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
            let path_hint = tool.input.get("path").and_then(|v| v.as_str()).unwrap_or("");
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
