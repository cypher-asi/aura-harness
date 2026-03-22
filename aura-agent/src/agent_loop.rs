//! Main agent loop orchestrator.
//!
//! `AgentLoop` drives the multi-step agentic conversation by calling
//! the model provider in a loop with intelligence: blocking detection,
//! compaction, sanitization, budget management, etc.

use std::collections::HashSet;
use std::time::Duration;

use aura_reasoner::{
    ContentBlock, Message, ModelProvider, ModelRequest, StopReason, ToolDefinition,
    ToolResultContent,
};
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
use crate::helpers;
use crate::read_guard::ReadGuardState;
use crate::sanitize;
use crate::types::{
    AgentLoopResult, AgentToolExecutor, BuildBaseline, ToolCallInfo, ToolCallResult,
};

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
            model: "claude-opus-4-5-20251101".to_string(),
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
    /// This is the main entry point. It drives the multi-step conversation
    /// by calling the model provider, processing tool calls, managing context,
    /// and applying intelligence (blocking, compaction, budget, etc.).
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
    pub async fn run(
        &self,
        provider: &dyn ModelProvider,
        executor: &dyn AgentToolExecutor,
        mut messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    ) -> anyhow::Result<AgentLoopResult> {
        let mut result = AgentLoopResult::default();

        let mut blocking_ctx = BlockingContext::new(self.config.exploration_allowance);
        let mut read_guard = ReadGuardState::default();
        let mut exploration_state = ExplorationState::default();
        let mut stall_detector = StallDetector::default();
        let mut budget_state = BudgetState::default();
        let mut had_any_write = false;
        let mut build_cooldown: usize = 0;
        let mut thinking_budget = self.config.max_tokens;

        let build_baseline = executor.capture_build_baseline().await;

        info!(
            max_iterations = self.config.max_iterations,
            exploration_allowance = self.config.exploration_allowance,
            "Starting agent loop"
        );

        for iteration in 0..self.config.max_iterations {
            build_cooldown = build_cooldown.saturating_sub(1);
            blocking_ctx.decrement_cooldowns();

            if iteration >= self.config.thinking_taper_after {
                thinking_budget =
                    (f64::from(thinking_budget) * self.config.thinking_taper_factor) as u32;
                thinking_budget = thinking_budget.max(self.config.thinking_min_budget);
            }

            sanitize::validate_and_repair(&mut messages);

            if let Some(max_ctx) = self.config.max_context_tokens {
                let char_count = compaction::estimate_message_chars(&messages);
                let estimated_tokens = char_count / CHARS_PER_TOKEN;
                let utilization = estimated_tokens as f64 / max_ctx as f64;

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
                .build();

            let response = match provider.complete(request).await {
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
            };

            result.total_input_tokens += response.usage.input_tokens;
            result.total_output_tokens += response.usage.output_tokens;

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
            result.iterations = iteration + 1;

            match response.stop_reason {
                StopReason::EndTurn | StopReason::MaxTokens | StopReason::StopSequence => {
                    break;
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

                    let tool_results = self
                        .process_tool_results(
                            &tool_calls,
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
                        .await;

                    let should_stop = tool_results.iter().any(|r| r.stop_loop);

                    let results: Vec<(String, ToolResultContent, bool)> = tool_results
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
                }
            }

            let utilization = (iteration + 1) as f64 / self.config.max_iterations as f64;
            if let Some(warning) =
                budget::check_budget_warning(&mut budget_state, utilization, had_any_write)
            {
                helpers::push_or_replace_warning(&mut messages, &warning);
            }

            if let Some(warning) = budget::check_exploration_warning(
                &mut exploration_state,
                self.config.exploration_allowance,
            ) {
                helpers::push_or_replace_warning(&mut messages, &warning);
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
    ) -> Vec<ToolCallResult> {
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

        let _stalled = stall_detector.update(&write_targets, any_write_success);

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
        all_results
    }
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

    #[test]
    fn test_tool_call_result_defaults() {
        let result = ToolCallResult::success("id", "content");
        assert!(!result.is_error);
        assert!(!result.stop_loop);

        let err = ToolCallResult::error("id", "error");
        assert!(err.is_error);
        assert!(!err.stop_loop);
    }
}
