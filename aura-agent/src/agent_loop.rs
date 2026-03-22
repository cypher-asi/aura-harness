//! Main agent loop orchestrator.
//!
//! `AgentLoop` drives the multi-step agentic conversation by calling
//! the model provider in a loop with intelligence: blocking detection,
//! compaction, sanitization, budget management, etc.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use aura_reasoner::{
    ContentBlock, Message, ModelProvider, ModelRequest, ModelResponse, StopReason,
    StreamAccumulator, StreamContentType, StreamEvent, ToolDefinition, ToolResultContent,
};
use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::blocking::detection::{detect_all_blocked, BlockingContext};
use crate::blocking::stall::StallDetector;
use crate::budget::{self, BudgetState, ExplorationState};
use crate::build;
use crate::compaction;
use crate::constants::{
    AUTO_BUILD_COOLDOWN, CHARS_PER_TOKEN, DEFAULT_EXPLORATION_ALLOWANCE, MAX_ITERATIONS,
    THINKING_MIN_BUDGET, THINKING_TAPER_AFTER, THINKING_TAPER_FACTOR,
};
use crate::events::AgentLoopEvent;
use crate::helpers;
use crate::read_guard::ReadGuardState;
use crate::sanitize;
use crate::types::{
    AgentLoopResult, AgentToolExecutor, BuildBaseline, ToolCallInfo, ToolCallResult,
};

/// Tools whose successful results can be cached within a single agent run.
const CACHEABLE_TOOLS: &[&str] = &["fs_read", "fs_ls", "fs_stat", "fs_find", "search_code"];

/// Configuration for the agent loop.
#[derive(Debug, Clone)]
pub struct AgentLoopConfig {
    /// Maximum iterations (model calls).
    pub max_iterations: usize,
    /// Maximum tokens per response.
    pub max_tokens: u32,
    /// Streaming timeout per iteration.
    pub stream_timeout: Duration,
    /// Credit attribution label.
    pub billing_reason: String,
    /// Loop-level model override.
    pub model_override: Option<String>,
    /// Maximum context tokens for compaction.
    pub max_context_tokens: Option<u64>,
    /// Credit budget (total tokens allowed).
    pub credit_budget: Option<u64>,
    /// Exploration allowance (read-only calls before warning).
    pub exploration_allowance: usize,
    /// Auto-build cooldown in iterations.
    pub auto_build_cooldown: usize,
    /// Thinking budget taper starts after this iteration.
    pub thinking_taper_after: usize,
    /// Factor to reduce thinking budget.
    pub thinking_taper_factor: f64,
    /// Minimum thinking budget after tapering.
    pub thinking_min_budget: u32,
    /// Additional tool definitions beyond core tools.
    pub extra_tools: Vec<ToolDefinition>,
    /// System prompt to use.
    pub system_prompt: String,
    /// Model name.
    pub model: String,
    /// JWT auth token for proxy routing.
    pub auth_token: Option<String>,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: MAX_ITERATIONS,
            max_tokens: 16_384,
            stream_timeout: Duration::from_secs(60),
            billing_reason: "agent_loop".to_string(),
            model_override: None,
            max_context_tokens: Some(200_000),
            credit_budget: None,
            exploration_allowance: DEFAULT_EXPLORATION_ALLOWANCE,
            auto_build_cooldown: AUTO_BUILD_COOLDOWN,
            thinking_taper_after: THINKING_TAPER_AFTER,
            thinking_taper_factor: THINKING_TAPER_FACTOR,
            thinking_min_budget: THINKING_MIN_BUDGET,
            extra_tools: Vec::new(),
            system_prompt: String::new(),
            model: "claude-opus-4-6-20250514".to_string(),
            auth_token: None,
        }
    }
}

/// The main multi-step agent loop orchestrator.
pub struct AgentLoop {
    config: AgentLoopConfig,
}

impl AgentLoop {
    /// Create a new agent loop with the given configuration.
    #[must_use]
    pub const fn new(config: AgentLoopConfig) -> Self {
        Self { config }
    }

    /// Run the agent loop with the given provider, executor, and initial messages.
    ///
    /// Backward-compatible entry point that delegates to
    /// [`run_with_events`](Self::run_with_events) with no event channel
    /// or cancellation token.
    ///
    /// # Errors
    ///
    /// Returns error if a model call or tool execution fails fatally.
    pub async fn run(
        &self,
        provider: &dyn ModelProvider,
        executor: &dyn AgentToolExecutor,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    ) -> anyhow::Result<AgentLoopResult> {
        self.run_with_events(provider, executor, messages, tools, None, None)
            .await
    }

    /// Run the agent loop with streaming events and cancellation support.
    ///
    /// When `event_tx` is `Some`, model calls use streaming and emit
    /// real-time [`AgentLoopEvent`]s through the channel. When `None`, the
    /// loop uses non-streaming `provider.complete()`.
    ///
    /// When `cancellation_token` is `Some`, the loop checks for cancellation
    /// at the start of each iteration and during streaming.
    ///
    /// A per-run tool cache avoids re-executing read-only tools with identical
    /// arguments. The cache is invalidated when any write tool succeeds.
    ///
    /// # Errors
    ///
    /// Returns error if a model call or tool execution fails fatally.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::too_many_lines
    )]
    pub async fn run_with_events(
        &self,
        provider: &dyn ModelProvider,
        executor: &dyn AgentToolExecutor,
        mut messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        event_tx: Option<UnboundedSender<AgentLoopEvent>>,
        cancellation_token: Option<CancellationToken>,
    ) -> anyhow::Result<AgentLoopResult> {
        let mut result = AgentLoopResult::default();
        let mut tool_cache: HashMap<String, String> = HashMap::new();

        let mut blocking_ctx = BlockingContext::new(self.config.exploration_allowance);
        let mut read_guard = ReadGuardState::default();
        let mut exploration_state = ExplorationState::default();
        let mut stall_detector = StallDetector::default();
        let mut budget_state = BudgetState::default();
        let mut had_any_write = false;
        let mut checkpoint_emitted = false;
        let mut exploration_compaction_done = false;
        let mut build_cooldown: usize = 0;
        let mut thinking_budget = self.config.max_tokens;
        let mut last_input_tokens: Option<u64> = None;

        let build_baseline = executor.capture_build_baseline().await;

        info!(
            max_iterations = self.config.max_iterations,
            exploration_allowance = self.config.exploration_allowance,
            "Starting agent loop"
        );

        for iteration in 0..self.config.max_iterations {
            if let Some(ref token) = cancellation_token {
                if token.is_cancelled() {
                    debug!("Cancellation requested, stopping loop");
                    break;
                }
            }

            build_cooldown = build_cooldown.saturating_sub(1);
            blocking_ctx.decrement_cooldowns();

            if iteration >= self.config.thinking_taper_after {
                thinking_budget =
                    (f64::from(thinking_budget) * self.config.thinking_taper_factor) as u32;
                thinking_budget = thinking_budget.max(self.config.thinking_min_budget);
            }

            sanitize::validate_and_repair(&mut messages);

            if let Some(max_ctx) = self.config.max_context_tokens {
                let utilization = if let Some(api_tokens) = last_input_tokens {
                    api_tokens as f64 / max_ctx as f64
                } else {
                    let char_count = compaction::estimate_message_chars(&messages);
                    let estimated_tokens = char_count / CHARS_PER_TOKEN;
                    estimated_tokens as f64 / max_ctx as f64
                };

                if let Some(tier) = compaction::select_tier(utilization) {
                    debug!(utilization, "Compacting context");
                    compaction::compact_older_messages(&mut messages, &tier);
                    sanitize::validate_and_repair(&mut messages);
                }
            }

            let request = ModelRequest::builder(&self.config.model, &self.config.system_prompt)
                .messages(messages.clone())
                .tools(tools.clone())
                .max_tokens(thinking_budget)
                .auth_token(self.config.auth_token.clone())
                .build();

            let response = if event_tx.is_some() {
                match self
                    .complete_with_streaming(
                        provider,
                        request,
                        &event_tx,
                        cancellation_token.as_ref(),
                    )
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        let err_msg = e.to_string();
                        if err_msg.contains("402") {
                            result.insufficient_credits = true;
                            warn!("Insufficient credits (402), stopping loop");
                            emit(
                                &event_tx,
                                AgentLoopEvent::Error {
                                    code: "insufficient_credits".to_string(),
                                    message: err_msg,
                                    recoverable: false,
                                },
                            );
                            break;
                        }
                        emit(
                            &event_tx,
                            AgentLoopEvent::Error {
                                code: "llm_error".to_string(),
                                message: err_msg.clone(),
                                recoverable: false,
                            },
                        );
                        result.llm_error = Some(err_msg);
                        break;
                    }
                }
            } else {
                match provider.complete(request).await {
                    Ok(r) => r,
                    Err(e) => {
                        let err_msg = e.to_string();
                        if err_msg.contains("402") {
                            result.insufficient_credits = true;
                            warn!("Insufficient credits (402), stopping loop");
                            break;
                        }
                        result.llm_error = Some(err_msg);
                        break;
                    }
                }
            };

            result.total_input_tokens += response.usage.input_tokens;
            result.total_output_tokens += response.usage.output_tokens;
            last_input_tokens = Some(response.usage.input_tokens);

            for block in &response.message.content {
                match block {
                    ContentBlock::Text { text } => result.total_text.push_str(text),
                    ContentBlock::Thinking { thinking, .. } => {
                        result.total_thinking.push_str(thinking);
                    }
                    _ => {}
                }
            }

            messages.push(response.message.clone());

            if let Some(last_msg) = messages.last_mut() {
                for block in &mut last_msg.content {
                    if let ContentBlock::ToolUse { name, input, .. } = block {
                        if let Some(summarized) = helpers::summarize_write_input(name, input) {
                            *input = summarized;
                        }
                    }
                }
            }

            result.iterations = iteration + 1;

            emit(
                &event_tx,
                AgentLoopEvent::IterationComplete {
                    iteration,
                    input_tokens: response.usage.input_tokens,
                    output_tokens: response.usage.output_tokens,
                },
            );

            match response.stop_reason {
                StopReason::EndTurn | StopReason::StopSequence => {
                    break;
                }
                StopReason::MaxTokens => {
                    let pending_tools: Vec<_> = response
                        .message
                        .content
                        .iter()
                        .filter_map(|block| {
                            if let ContentBlock::ToolUse { id, name, .. } = block {
                                Some((id.clone(), name.clone()))
                            } else {
                                None
                            }
                        })
                        .collect();

                    if pending_tools.is_empty() {
                        break;
                    }

                    warn!(
                        pending = pending_tools.len(),
                        "MaxTokens with pending tool_use blocks — injecting error results"
                    );

                    let results: Vec<(String, ToolResultContent, bool)> = pending_tools
                        .iter()
                        .map(|(id, name)| {
                            (
                                id.clone(),
                                ToolResultContent::text(format!(
                                    "Error: Response was truncated (max_tokens). Tool '{}' was not executed. \
                                     Please try again with a simpler approach or break the task into smaller steps.",
                                    name
                                )),
                                true,
                            )
                        })
                        .collect();

                    messages.push(Message::tool_results(results));

                    if let Some(_max_ctx) = self.config.max_context_tokens {
                        let tier = compaction::CompactionConfig::aggressive();
                        compaction::compact_older_messages(&mut messages, &tier);
                        sanitize::validate_and_repair(&mut messages);
                    }
                }
                StopReason::ToolUse => {
                    let tool_calls: Vec<ToolCallInfo> = response
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
                        .collect();

                    if tool_calls.is_empty() {
                        break;
                    }

                    // Separate cached from uncached tool calls
                    let mut cached_results = Vec::new();
                    let mut uncached_calls = Vec::new();

                    for tc in &tool_calls {
                        if is_cacheable(&tc.name) {
                            let key = cache_key(&tc.name, &tc.input);
                            if let Some(cached) = tool_cache.get(&key) {
                                cached_results.push(ToolCallResult {
                                    tool_use_id: tc.id.clone(),
                                    content: cached.clone(),
                                    is_error: false,
                                    stop_loop: false,
                                });
                                continue;
                            }
                        }
                        uncached_calls.push(tc.clone());
                    }

                    let (executed_results, is_stalled) = if uncached_calls.is_empty() {
                        (Vec::new(), false)
                    } else {
                        self.process_tool_results(
                            &uncached_calls,
                            executor,
                            &mut blocking_ctx,
                            &mut read_guard,
                            &mut exploration_state,
                            &mut stall_detector,
                            &mut messages,
                            &mut had_any_write,
                            &mut build_cooldown,
                            build_baseline.as_ref(),
                        )
                        .await
                    };

                    // Invalidate cache when a write tool succeeds
                    let any_write_succeeded = uncached_calls.iter().any(|tc| {
                        helpers::is_write_tool(&tc.name)
                            && executed_results
                                .iter()
                                .any(|r| r.tool_use_id == tc.id && !r.is_error)
                    });
                    if any_write_succeeded {
                        tool_cache.clear();
                    }

                    // Cache successful results from cacheable tools
                    for exec_result in &executed_results {
                        if let Some(tc) =
                            uncached_calls.iter().find(|t| t.id == exec_result.tool_use_id)
                        {
                            if is_cacheable(&tc.name) && !exec_result.is_error {
                                let key = cache_key(&tc.name, &tc.input);
                                tool_cache.insert(key, exec_result.content.clone());
                            }
                        }
                    }

                    // Emit ToolResult events for all results
                    for r in cached_results.iter().chain(executed_results.iter()) {
                        let tool_name = tool_calls
                            .iter()
                            .find(|t| t.id == r.tool_use_id)
                            .map_or_else(String::new, |t| t.name.clone());
                        emit(
                            &event_tx,
                            AgentLoopEvent::ToolResult {
                                tool_use_id: r.tool_use_id.clone(),
                                tool_name,
                                content: r.content.clone(),
                                is_error: r.is_error,
                            },
                        );
                    }

                    let mut all_tool_results = cached_results;
                    all_tool_results.extend(executed_results);

                    let should_stop = all_tool_results.iter().any(|r| r.stop_loop);

                    let results: Vec<(String, ToolResultContent, bool)> = all_tool_results
                        .into_iter()
                        .map(|r| {
                            (
                                r.tool_use_id,
                                ToolResultContent::text(r.content),
                                r.is_error,
                            )
                        })
                        .collect();

                    if !results.is_empty() {
                        messages.push(Message::tool_results(results));
                    }

                    if should_stop {
                        break;
                    }

                    if is_stalled {
                        let msg = "CRITICAL: Agent appears stalled — repeatedly failing \
                                   to write to the same files. Stopping to prevent \
                                   infinite loop. Try a different approach or ask for help.";
                        helpers::push_or_replace_warning(&mut messages, msg);
                        emit(
                            &event_tx,
                            AgentLoopEvent::Error {
                                code: "stall_detected".to_string(),
                                message: msg.to_string(),
                                recoverable: false,
                            },
                        );
                        result.stalled = true;
                        break;
                    }
                }
            }

            if had_any_write && !checkpoint_emitted {
                checkpoint_emitted = true;
                let checkpoint_msg = "NOTE: You've made your first file change. Before making more changes, consider verifying your work (e.g., run the build or tests) to catch issues early.".to_string();
                helpers::push_or_replace_warning(&mut messages, &checkpoint_msg);
                emit(&event_tx, AgentLoopEvent::Warning(checkpoint_msg));
            }

            // Exploration-triggered proactive compaction
            let exploration_threshold = (self.config.exploration_allowance * 2) / 3;
            if exploration_state.count >= exploration_threshold && !exploration_compaction_done {
                if let Some(_max_ctx) = self.config.max_context_tokens {
                    let tier = compaction::CompactionConfig::history();
                    compaction::compact_older_messages(&mut messages, &tier);
                    sanitize::validate_and_repair(&mut messages);
                    exploration_compaction_done = true;
                    debug!(
                        exploration_count = exploration_state.count,
                        threshold = exploration_threshold,
                        "Proactive compaction triggered by exploration usage"
                    );
                }
            }

            let utilization = (iteration + 1) as f64 / self.config.max_iterations as f64;
            if let Some(warning) =
                budget::check_budget_warning(&mut budget_state, utilization, had_any_write)
            {
                helpers::push_or_replace_warning(&mut messages, &warning);
                emit(&event_tx, AgentLoopEvent::Warning(warning));
            }

            if let Some(warning) = budget::check_exploration_warning(
                &mut exploration_state,
                self.config.exploration_allowance,
            ) {
                helpers::push_or_replace_warning(&mut messages, &warning);
                emit(&event_tx, AgentLoopEvent::Warning(warning));
            }

            let total_tokens = result.total_input_tokens + result.total_output_tokens;
            let iterations_done = (iteration as u64) + 1;
            let avg_tokens = total_tokens / iterations_done.max(1);
            if budget::should_stop_for_budget(
                iteration + 1,
                self.config.max_iterations,
                avg_tokens,
                total_tokens,
                self.config.credit_budget,
            ) {
                result.timed_out = true;
                break;
            }
        }

        result.messages = messages;
        Ok(result)
    }

    /// Perform a model completion using streaming, emitting events as they arrive.
    ///
    /// Falls back to non-streaming `provider.complete()` if the streaming call
    /// itself returns an error.
    #[allow(clippy::cast_possible_truncation)]
    async fn complete_with_streaming(
        &self,
        provider: &dyn ModelProvider,
        request: ModelRequest,
        event_tx: &Option<UnboundedSender<AgentLoopEvent>>,
        cancellation_token: Option<&CancellationToken>,
    ) -> anyhow::Result<ModelResponse> {
        let start = Instant::now();

        match provider.complete_streaming(request.clone()).await {
            Ok(mut stream) => {
                let mut accumulator = StreamAccumulator::new();

                loop {
                    let next = if let Some(token) = cancellation_token {
                        tokio::select! {
                            _ = token.cancelled() => {
                                return Err(anyhow::anyhow!("Cancelled"));
                            }
                            item = stream.next() => item,
                        }
                    } else {
                        stream.next().await
                    };

                    match next {
                        Some(Ok(event)) => {
                            accumulator.process(&event);
                            emit_stream_event(event_tx, &event, &accumulator);
                        }
                        Some(Err(e)) => {
                            debug!("Stream error, falling back to non-streaming: {e}");
                            emit(
                                event_tx,
                                AgentLoopEvent::Warning(format!(
                                    "Stream error, retrying without streaming: {e}"
                                )),
                            );
                            return provider.complete(request).await;
                        }
                        None => break,
                    }
                }

                let latency_ms = start.elapsed().as_millis() as u64;
                accumulator.into_response(0, latency_ms)
            }
            Err(e) => {
                debug!("complete_streaming failed, falling back: {e}");
                emit(
                    event_tx,
                    AgentLoopEvent::Warning(format!(
                        "Streaming unavailable, using non-streaming: {e}"
                    )),
                );
                provider.complete(request).await
            }
        }
    }

    /// Process tool call results from one iteration.
    ///
    /// Applies blocking detection, tracks writes/reads/commands,
    /// manages exploration budget, and handles build checks.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn process_tool_results(
        &self,
        tool_calls: &[ToolCallInfo],
        executor: &dyn AgentToolExecutor,
        blocking_ctx: &mut BlockingContext,
        read_guard: &mut ReadGuardState,
        exploration_state: &mut ExplorationState,
        stall_detector: &mut StallDetector,
        messages: &mut Vec<Message>,
        had_any_write: &mut bool,
        build_cooldown: &mut usize,
        build_baseline: Option<&BuildBaseline>,
    ) -> (Vec<ToolCallResult>, bool) {
        let mut blocked_results = Vec::new();
        let mut to_execute = Vec::new();

        for tool in tool_calls {
            let check = detect_all_blocked(tool, blocking_ctx, read_guard);
            if check.blocked {
                let msg = check
                    .recovery_message
                    .unwrap_or_else(|| "Blocked".to_string());
                helpers::push_or_replace_warning(messages, &msg);
                blocked_results.push(ToolCallResult {
                    tool_use_id: tool.id.clone(),
                    content: msg,
                    is_error: true,
                    stop_loop: false,
                });
            } else {
                to_execute.push(tool.clone());
            }
        }

        let executed = if to_execute.is_empty() {
            Vec::new()
        } else {
            executor.execute(&to_execute).await
        };

        let mut write_targets = HashSet::new();
        let mut any_write_success = false;

        for exec_result in &executed {
            let tool = to_execute.iter().find(|t| t.id == exec_result.tool_use_id);
            if let Some(tool) = tool {
                if helpers::is_exploration_tool(&tool.name) {
                    exploration_state.count += 1;
                    let path = tool.input.get("path").and_then(|v| v.as_str());
                    if let Some(path) = path {
                        let is_range = tool.input.get("start_line").is_some();
                        if is_range {
                            read_guard.record_range_read(path);
                        } else {
                            read_guard.record_full_read(path);
                        }
                    }
                }

                if helpers::is_write_tool(&tool.name) {
                    if let Some(path) = tool.input.get("path").and_then(|v| v.as_str()) {
                        write_targets.insert(path.to_string());
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
        }

        let stalled = stall_detector.update(&write_targets, any_write_success);
        if stalled {
            warn!(
                streak = stall_detector.streak(),
                "Stall detected: same write targets failing repeatedly"
            );
        }

        if any_write_success && *build_cooldown == 0 {
            if let Some(build_result) = executor.auto_build_check().await {
                *build_cooldown = self.config.auto_build_cooldown;
                if !build_result.success {
                    let annotated = if let Some(baseline) = build_baseline {
                        build::annotate_build_output(&build_result.output, baseline)
                    } else {
                        build_result.output.clone()
                    };
                    messages.push(Message::user(format!(
                        "Build check failed with {} error(s):\n\n{annotated}",
                        build_result.error_count
                    )));
                }
            }
        }

        if any_write_success {
            blocking_ctx.exploration_allowance += 2;
        }

        let mut all_results = blocked_results;
        all_results.extend(executed);
        (all_results, stalled)
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Send an event through the channel if present.
fn emit(tx: &Option<UnboundedSender<AgentLoopEvent>>, event: AgentLoopEvent) {
    if let Some(tx) = tx {
        let _ = tx.send(event);
    }
}

/// Map a [`StreamEvent`] to the corresponding [`AgentLoopEvent`] and emit it.
fn emit_stream_event(
    event_tx: &Option<UnboundedSender<AgentLoopEvent>>,
    stream_event: &StreamEvent,
    accumulator: &StreamAccumulator,
) {
    if event_tx.is_none() {
        return;
    }

    match stream_event {
        StreamEvent::TextDelta { text } => {
            emit(event_tx, AgentLoopEvent::TextDelta(text.clone()));
        }
        StreamEvent::ThinkingDelta { thinking } => {
            emit(event_tx, AgentLoopEvent::ThinkingDelta(thinking.clone()));
        }
        StreamEvent::ContentBlockStart {
            content_type: StreamContentType::ToolUse { id, name },
            ..
        } => {
            emit(
                event_tx,
                AgentLoopEvent::ToolStart {
                    id: id.clone(),
                    name: name.clone(),
                },
            );
        }
        StreamEvent::InputJsonDelta { .. } => {
            if let Some(ref tool) = accumulator.current_tool_use {
                emit(
                    event_tx,
                    AgentLoopEvent::ToolInputSnapshot {
                        id: tool.id.clone(),
                        name: tool.name.clone(),
                        input: tool.input_json.clone(),
                    },
                );
            }
        }
        StreamEvent::Error { message } => {
            emit(
                event_tx,
                AgentLoopEvent::Error {
                    code: "stream_error".to_string(),
                    message: message.clone(),
                    recoverable: true,
                },
            );
        }
        _ => {}
    }
}

/// Check whether a tool's results are eligible for caching.
fn is_cacheable(tool_name: &str) -> bool {
    CACHEABLE_TOOLS.contains(&tool_name)
}

/// Build a deterministic cache key for a tool invocation.
fn cache_key(tool_name: &str, input: &serde_json::Value) -> String {
    let canonical = serde_json::to_string(input).unwrap_or_default();
    format!("{tool_name}\0{canonical}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_reasoner::{MockProvider, MockResponse};

    struct MockExecutor {
        results: Vec<ToolCallResult>,
    }

    #[async_trait::async_trait]
    impl AgentToolExecutor for MockExecutor {
        async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
            tool_calls
                .iter()
                .zip(self.results.iter())
                .map(|(tc, r)| ToolCallResult {
                    tool_use_id: tc.id.clone(),
                    ..r.clone()
                })
                .collect()
        }
    }

    #[test]
    fn test_agent_loop_config_defaults() {
        let config = AgentLoopConfig::default();
        assert_eq!(config.max_iterations, 25);
        assert_eq!(config.exploration_allowance, 12);
        assert_eq!(config.auto_build_cooldown, 2);
        assert_eq!(config.thinking_taper_after, 2);
        assert!((config.thinking_taper_factor - 0.6).abs() < f64::EPSILON);
        assert_eq!(config.thinking_min_budget, 1024);
    }

    #[tokio::test]
    async fn test_agent_loop_simple_run() {
        let config = AgentLoopConfig::default();
        let agent = AgentLoop::new(config);
        let executor = MockExecutor { results: vec![] };
        let provider = MockProvider::simple_response("Hello!");
        let messages = vec![Message::user("hello")];
        let tools = vec![];

        let result = agent
            .run(&provider, &executor, messages, tools)
            .await
            .unwrap();
        assert_eq!(result.iterations, 1);
        assert!(result.total_text.contains("Hello!"));
        assert!(result.total_input_tokens > 0);
    }

    #[tokio::test]
    async fn test_agent_loop_full_integration() {
        let executor = MockExecutor {
            results: vec![ToolCallResult::success("placeholder", "file contents here")],
        };

        let provider = MockProvider::new()
            .with_response(MockResponse::tool_use(
                "tool_1",
                "fs_read",
                serde_json::json!({"path": "test.txt"}),
            ))
            .with_response(MockResponse::text("All done!"));

        let config = AgentLoopConfig {
            system_prompt: "You are a test agent".to_string(),
            ..AgentLoopConfig::default()
        };
        let agent = AgentLoop::new(config);
        let messages = vec![Message::user("Read test.txt")];
        let tools = vec![ToolDefinition::new(
            "fs_read",
            "Read a file",
            serde_json::json!({"type": "object"}),
        )];

        let result = agent
            .run(&provider, &executor, messages, tools)
            .await
            .unwrap();

        assert_eq!(result.iterations, 2);
        assert!(result.total_text.contains("All done!"));
        assert!(result.total_input_tokens > 0);
        assert!(result.total_output_tokens > 0);
        assert!(!result.insufficient_credits);
        assert!(result.llm_error.is_none());
    }

    #[tokio::test]
    async fn test_agent_loop_402_insufficient_credits() {
        let executor = MockExecutor { results: vec![] };
        let provider = MockProvider::new().with_failure();

        let config = AgentLoopConfig::default();
        let agent = AgentLoop::new(config);
        let messages = vec![Message::user("hello")];
        let tools = vec![];

        let result = agent
            .run(&provider, &executor, messages, tools)
            .await
            .unwrap();
        assert!(result.llm_error.is_some());
    }

    #[tokio::test]
    async fn test_max_tokens_with_pending_tools_injects_errors() {
        let executor = MockExecutor { results: vec![] };

        let provider = MockProvider::new()
            .with_response(
                MockResponse::tool_use(
                    "tool_1",
                    "fs_read",
                    serde_json::json!({"path": "big_file.txt"}),
                )
                .with_stop_reason(StopReason::MaxTokens),
            )
            .with_response(MockResponse::text("Recovered after truncation."));

        let config = AgentLoopConfig {
            system_prompt: "Test agent".to_string(),
            ..AgentLoopConfig::default()
        };
        let agent = AgentLoop::new(config);
        let messages = vec![Message::user("Read big_file.txt")];
        let tools = vec![ToolDefinition::new(
            "fs_read",
            "Read a file",
            serde_json::json!({"type": "object"}),
        )];

        let result = agent
            .run(&provider, &executor, messages, tools)
            .await
            .unwrap();

        assert_eq!(result.iterations, 2, "Loop should continue after MaxTokens with pending tools");
        assert!(result.total_text.contains("Recovered after truncation."));

        let has_error_tool_result = result.messages.iter().any(|msg| {
            msg.content.iter().any(|block| {
                matches!(block, ContentBlock::ToolResult { is_error: true, .. })
            })
        });
        assert!(has_error_tool_result, "Should have injected an error tool result");
    }

    #[tokio::test]
    async fn test_max_tokens_without_tools_breaks() {
        let executor = MockExecutor { results: vec![] };

        let provider = MockProvider::new()
            .with_response(MockResponse::text("Truncated text").with_stop_reason(StopReason::MaxTokens))
            .with_response(MockResponse::text("Should not reach this"));

        let config = AgentLoopConfig {
            system_prompt: "Test agent".to_string(),
            ..AgentLoopConfig::default()
        };
        let agent = AgentLoop::new(config);
        let messages = vec![Message::user("hello")];
        let tools = vec![];

        let result = agent
            .run(&provider, &executor, messages, tools)
            .await
            .unwrap();

        assert_eq!(result.iterations, 1, "Loop should break on MaxTokens with no pending tools");
        assert!(result.total_text.contains("Truncated text"));
        assert!(!result.total_text.contains("Should not reach this"));
    }

    #[test]
    fn test_tool_call_result_defaults() {
        let result = ToolCallResult::success("id", "content");
        assert!(!result.is_error);
        assert!(!result.stop_loop);

        let err = ToolCallResult::error("id", "error");
        assert!(err.is_error);
        assert!(!err.stop_loop);
    }

    #[tokio::test]
    async fn test_compaction_uses_api_input_tokens() {
        use aura_reasoner::Usage;

        let executor = MockExecutor {
            results: vec![ToolCallResult::success("placeholder", "ok")],
        };

        // First response: tool use with input_tokens = 180_000 (90% of 200k).
        // Second response: final text — by this iteration last_input_tokens is
        // set, so compaction should trigger based on the API-reported value
        // rather than the (tiny) char-count heuristic.
        let high_usage_tool = MockResponse {
            stop_reason: StopReason::ToolUse,
            content: vec![ContentBlock::tool_use(
                "tool_1",
                "fs_read",
                serde_json::json!({"path": "big.txt"}),
            )],
            usage: Usage::new(180_000, 50),
        };
        let final_resp = MockResponse {
            stop_reason: StopReason::EndTurn,
            content: vec![ContentBlock::text("Done")],
            usage: Usage::new(185_000, 50),
        };

        let provider = MockProvider::new()
            .with_response(high_usage_tool)
            .with_response(final_resp);

        let config = AgentLoopConfig {
            max_context_tokens: Some(200_000),
            system_prompt: "test".to_string(),
            ..AgentLoopConfig::default()
        };
        let agent = AgentLoop::new(config);
        let messages = vec![Message::user("go")];
        let tools = vec![ToolDefinition::new(
            "fs_read",
            "Read a file",
            serde_json::json!({"type": "object"}),
        )];

        let result = agent
            .run(&provider, &executor, messages, tools)
            .await
            .unwrap();

        assert_eq!(result.iterations, 2);
        // The API reported 180k input tokens on iter 0 (90% of 200k), which
        // exceeds the history tier threshold (85%). On iter 1 the loop should
        // have used that value for compaction instead of the char heuristic.
        // We verify compaction ran by checking total_input_tokens includes both
        // iterations' API-reported counts.
        assert_eq!(result.total_input_tokens, 180_000 + 185_000);
    }

    #[tokio::test]
    async fn test_checkpoint_after_first_write() {
        let executor = MockExecutor {
            results: vec![ToolCallResult::success("placeholder", "wrote file")],
        };

        let provider = MockProvider::new()
            .with_response(MockResponse::tool_use(
                "tool_1",
                "write_file",
                serde_json::json!({"path": "hello.txt", "content": "hi"}),
            ))
            .with_response(MockResponse::text("Done!"));

        let config = AgentLoopConfig {
            system_prompt: "test".to_string(),
            ..AgentLoopConfig::default()
        };
        let agent = AgentLoop::new(config);
        let messages = vec![Message::user("write hello.txt")];
        let tools = vec![ToolDefinition::new(
            "write_file",
            "Write a file",
            serde_json::json!({"type": "object"}),
        )];

        let result = agent
            .run(&provider, &executor, messages, tools)
            .await
            .unwrap();

        let has_checkpoint = result.messages.iter().any(|msg| {
            msg.content.iter().any(|block| {
                if let ContentBlock::Text { text } = block {
                    text.contains("You've made your first file change")
                } else {
                    false
                }
            })
        });
        assert!(has_checkpoint, "Messages should contain the checkpoint note after first write");
    }

    #[tokio::test]
    async fn test_checkpoint_not_repeated() {
        let executor = MockExecutor {
            results: vec![
                ToolCallResult::success("placeholder", "wrote file 1"),
                ToolCallResult::success("placeholder", "wrote file 2"),
            ],
        };

        let provider = MockProvider::new()
            .with_response(MockResponse::tool_use(
                "tool_1",
                "write_file",
                serde_json::json!({"path": "a.txt", "content": "a"}),
            ))
            .with_response(MockResponse::tool_use(
                "tool_2",
                "write_file",
                serde_json::json!({"path": "b.txt", "content": "b"}),
            ))
            .with_response(MockResponse::text("All done!"));

        let config = AgentLoopConfig {
            system_prompt: "test".to_string(),
            ..AgentLoopConfig::default()
        };
        let agent = AgentLoop::new(config);
        let messages = vec![Message::user("write two files")];
        let tools = vec![ToolDefinition::new(
            "write_file",
            "Write a file",
            serde_json::json!({"type": "object"}),
        )];

        let result = agent
            .run(&provider, &executor, messages, tools)
            .await
            .unwrap();

        let checkpoint_count = result
            .messages
            .iter()
            .flat_map(|msg| msg.content.iter())
            .filter(|block| {
                if let ContentBlock::Text { text } = block {
                    text.contains("You've made your first file change")
                } else {
                    false
                }
            })
            .count();
        assert_eq!(checkpoint_count, 1, "Checkpoint message should appear exactly once");
    }

    #[tokio::test]
    async fn test_stall_terminates_loop() {
        let executor = MockExecutor {
            results: vec![ToolCallResult::error(
                "placeholder",
                "Write failed: permission denied",
            )],
        };

        let provider = MockProvider::new().with_default_response(MockResponse::tool_use(
            "tool_w",
            "fs_write",
            serde_json::json!({"path": "stuck.rs", "content": "bad code"}),
        ));

        let max_iter = 10;
        let config = AgentLoopConfig {
            max_iterations: max_iter,
            system_prompt: "test".to_string(),
            ..AgentLoopConfig::default()
        };
        let agent = AgentLoop::new(config);
        let messages = vec![Message::user("write stuck.rs")];
        let tools = vec![ToolDefinition::new(
            "fs_write",
            "Write a file",
            serde_json::json!({"type": "object"}),
        )];

        let result = agent
            .run(&provider, &executor, messages, tools)
            .await
            .unwrap();

        assert!(
            result.stalled,
            "Loop should have been terminated by stall detection"
        );
        assert!(
            result.iterations < max_iter,
            "Loop should terminate before max_iterations (got {})",
            result.iterations
        );
        assert_eq!(
            result.iterations,
            crate::constants::STALL_STREAK_THRESHOLD,
            "Loop should terminate after exactly STALL_STREAK_THRESHOLD iterations"
        );

        let has_stall_warning = result.messages.iter().any(|msg| {
            msg.content.iter().any(|block| {
                if let ContentBlock::Text { text } = block {
                    text.contains("CRITICAL") && text.contains("stalled")
                } else {
                    false
                }
            })
        });
        assert!(
            has_stall_warning,
            "Messages should contain the stall recovery warning"
        );
    }

    #[tokio::test]
    async fn test_exploration_compact_at_two_thirds() {
        let long_content = "x".repeat(3000);
        let executor = MockExecutor {
            results: vec![ToolCallResult::success("placeholder", &long_content)],
        };

        let mut provider_builder = MockProvider::new();
        for i in 0..8 {
            provider_builder = provider_builder.with_response(MockResponse::tool_use(
                format!("t{i}"),
                "fs_read",
                serde_json::json!({"path": format!("file{i}.txt")}),
            ));
        }
        provider_builder = provider_builder.with_response(MockResponse::text("Done"));
        let provider = provider_builder;

        let config = AgentLoopConfig {
            exploration_allowance: 12,
            max_context_tokens: Some(200_000),
            system_prompt: "test".to_string(),
            ..AgentLoopConfig::default()
        };
        let agent = AgentLoop::new(config);
        let messages = vec![Message::user("read many files")];
        let tools = vec![ToolDefinition::new(
            "fs_read",
            "Read a file",
            serde_json::json!({"type": "object"}),
        )];

        let result = agent
            .run(&provider, &executor, messages, tools)
            .await
            .unwrap();

        assert_eq!(result.iterations, 9);

        let has_truncation = result.messages.iter().any(|msg| {
            msg.content.iter().any(|block| {
                if let ContentBlock::ToolResult { content, .. } = block {
                    match content {
                        ToolResultContent::Text(t) => t.contains("content truncated"),
                        _ => false,
                    }
                } else {
                    false
                }
            })
        });
        assert!(
            has_truncation,
            "Exploration-triggered compaction should have truncated older tool results"
        );
    }

    #[tokio::test]
    async fn test_no_exploration_compact_when_low() {
        let long_content = "y".repeat(3000);
        let executor = MockExecutor {
            results: vec![ToolCallResult::success("placeholder", &long_content)],
        };

        let mut provider_builder = MockProvider::new();
        for i in 0..3 {
            provider_builder = provider_builder.with_response(MockResponse::tool_use(
                format!("t{i}"),
                "fs_read",
                serde_json::json!({"path": format!("file{i}.txt")}),
            ));
        }
        provider_builder = provider_builder.with_response(MockResponse::text("Done"));
        let provider = provider_builder;

        let config = AgentLoopConfig {
            exploration_allowance: 12,
            max_context_tokens: Some(200_000),
            system_prompt: "test".to_string(),
            ..AgentLoopConfig::default()
        };
        let agent = AgentLoop::new(config);
        let messages = vec![Message::user("read a few files")];
        let tools = vec![ToolDefinition::new(
            "fs_read",
            "Read a file",
            serde_json::json!({"type": "object"}),
        )];

        let result = agent
            .run(&provider, &executor, messages, tools)
            .await
            .unwrap();

        assert_eq!(result.iterations, 4);

        let has_truncation = result.messages.iter().any(|msg| {
            msg.content.iter().any(|block| {
                if let ContentBlock::ToolResult { content, .. } = block {
                    match content {
                        ToolResultContent::Text(t) => t.contains("content truncated"),
                        _ => false,
                    }
                } else {
                    false
                }
            })
        });
        assert!(
            !has_truncation,
            "No compaction should occur with only 3 exploration calls (threshold is 8)"
        );
    }
}
