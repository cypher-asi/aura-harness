//! Main agent loop orchestrator.
//!
//! `AgentLoop` drives the multi-step agentic conversation by calling
//! the model provider in a loop with intelligence: blocking detection,
//! compaction, sanitization, budget management, etc.

mod context;
mod iteration;
mod streaming;
mod tool_execution;
#[cfg(test)]
mod tool_execution_tests;
mod tool_processing;

#[cfg(test)]
mod contract_tests;
#[cfg(test)]
mod parity_tests;
#[cfg(test)]
mod cutover_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_advanced;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::runtime::ModelCallDelegate;
use aura_reasoner::{Message, ModelProvider, ModelRequest, StopReason, ToolDefinition};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::blocking::detection::BlockingContext;
use crate::blocking::stall::StallDetector;
use crate::budget::{BudgetState, ExplorationState};
use crate::constants::{
    AUTO_BUILD_COOLDOWN, DEFAULT_EXPLORATION_ALLOWANCE, MAX_ITERATIONS, THINKING_MIN_BUDGET,
    THINKING_TAPER_AFTER, THINKING_TAPER_FACTOR,
};
use crate::events::AgentLoopEvent;
use crate::read_guard::ReadGuardState;
use crate::types::{AgentLoopResult, AgentToolExecutor, BuildBaseline};

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
    /// Tool names the user wants prioritized for the current turn.
    /// On the first iteration, tools are filtered to this set and
    /// `tool_choice` is set to force tool usage.
    pub tool_hints: Option<Vec<String>>,
    /// Project ID for X-Aura-Project-Id billing header.
    pub aura_project_id: Option<String>,
    /// Project-agent UUID for X-Aura-Agent-Id billing header.
    pub aura_agent_id: Option<String>,
    /// Storage session UUID for X-Aura-Session-Id billing header.
    pub aura_session_id: Option<String>,
    /// Org UUID for X-Aura-Org-Id billing header.
    pub aura_org_id: Option<String>,
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
            model: crate::constants::DEFAULT_MODEL.to_string(),
            auth_token: None,
            tool_hints: None,
            aura_project_id: None,
            aura_agent_id: None,
            aura_session_id: None,
            aura_org_id: None,
        }
    }
}

/// The main multi-step agent loop orchestrator.
pub struct AgentLoop {
    config: AgentLoopConfig,
    /// Optional delegate for model calls. When set, the loop delegates
    /// model completions to this processor instead of calling the
    /// `ModelProvider` directly, gaining the delegate's streaming,
    /// cancellation, and replay infrastructure.
    model_delegate: Option<Arc<dyn ModelCallDelegate>>,
}

impl AgentLoop {
    /// Create a new agent loop with the given configuration.
    #[must_use]
    pub fn new(config: AgentLoopConfig) -> Self {
        Self {
            config,
            model_delegate: None,
        }
    }

    /// Set a [`ModelCallDelegate`] to handle model calls.
    ///
    /// When a delegate is set, [`AgentLoop`] routes all model completions
    /// through `delegate.call_model()` instead of calling the provider
    /// directly. The delegate handles streaming, cancellation, and replay
    /// internally.
    ///
    /// **Streaming note:** Per-token streaming events (text deltas, thinking
    /// deltas) are emitted by the delegate's own callback. Configure
    /// streaming on the delegate before passing it here. Higher-level
    /// events (`IterationComplete`, `ToolResult`, `Error`) are still
    /// emitted through `event_tx`.
    #[must_use]
    pub fn with_model_delegate(mut self, delegate: Arc<dyn ModelCallDelegate>) -> Self {
        self.model_delegate = Some(delegate);
        self
    }

    /// Update the auth token for subsequent model requests.
    pub fn set_auth_token(&mut self, token: Option<String>) {
        self.config.auth_token = token;
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
    ) -> Result<AgentLoopResult, crate::AgentError> {
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
    pub async fn run_with_events(
        &self,
        provider: &dyn ModelProvider,
        executor: &dyn AgentToolExecutor,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        event_tx: Option<Sender<AgentLoopEvent>>,
        cancellation_token: Option<CancellationToken>,
    ) -> Result<AgentLoopResult, crate::AgentError> {
        let mut state = LoopState::new(&self.config, messages);
        state.build_baseline = executor.capture_build_baseline().await;
        info!(
            max_iterations = self.config.max_iterations,
            exploration_allowance = self.config.exploration_allowance,
            "Starting agent loop"
        );

        for iteration in 0..self.config.max_iterations {
            if is_cancelled(cancellation_token.as_ref()) {
                debug!("Cancellation requested, stopping loop");
                break;
            }
            state.begin_iteration(&self.config, iteration);
            context::compact_if_needed(&self.config, &mut state);

            let request = state.build_request(&self.config, &tools, iteration);
            let response = match self
                .call_model(
                    provider,
                    request,
                    event_tx.as_ref(),
                    cancellation_token.as_ref(),
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    e.apply(&mut state.result, event_tx.as_ref());
                    break;
                }
            };

            iteration::accumulate_response(&mut state, &response);
            state.result.iterations = iteration + 1;
            streaming::emit_iteration_complete(event_tx.as_ref(), iteration, &response);

            if self
                .dispatch_stop_reason(&response, executor, event_tx.as_ref(), &mut state)
                .await
            {
                break;
            }
            if post_iteration_checks(&self.config, event_tx.as_ref(), &mut state, iteration) {
                break;
            }
        }

        state.result.messages = state.messages;
        Ok(state.result)
    }

    /// Dispatch on the model's stop reason. Returns `true` if the loop should break.
    async fn dispatch_stop_reason(
        &self,
        response: &aura_reasoner::ModelResponse,
        executor: &dyn AgentToolExecutor,
        event_tx: Option<&Sender<AgentLoopEvent>>,
        state: &mut LoopState,
    ) -> bool {
        match response.stop_reason {
            StopReason::EndTurn | StopReason::StopSequence => true,
            StopReason::MaxTokens => !iteration::handle_max_tokens(&self.config, response, state),
            StopReason::ToolUse => {
                tool_execution::handle_tool_use(self, response, executor, event_tx, state).await
            }
        }
    }
}

/// Mutable state carried across iterations of the agent loop.
pub struct LoopState {
    pub(crate) result: AgentLoopResult,
    pub(crate) tool_cache: HashMap<String, String>,
    pub(crate) blocking_ctx: BlockingContext,
    pub(crate) read_guard: ReadGuardState,
    pub(crate) exploration_state: ExplorationState,
    pub(crate) stall_detector: StallDetector,
    pub(crate) budget_state: BudgetState,
    pub(crate) had_any_write: bool,
    pub(crate) checkpoint_emitted: bool,
    pub(crate) exploration_compaction_done: bool,
    pub(crate) build_cooldown: usize,
    pub(crate) thinking_budget: u32,
    pub(crate) last_input_tokens: Option<u64>,
    pub(crate) messages: Vec<Message>,
    pub(crate) build_baseline: Option<BuildBaseline>,
    /// Consecutive iterations where every tool call returned an error.
    pub(crate) consecutive_all_error_iterations: usize,
}

impl LoopState {
    fn new(config: &AgentLoopConfig, messages: Vec<Message>) -> Self {
        Self {
            result: AgentLoopResult::default(),
            tool_cache: HashMap::new(),
            blocking_ctx: BlockingContext::new(config.exploration_allowance),
            read_guard: ReadGuardState::default(),
            exploration_state: ExplorationState::default(),
            stall_detector: StallDetector::default(),
            budget_state: BudgetState::default(),
            had_any_write: false,
            checkpoint_emitted: false,
            exploration_compaction_done: false,
            build_cooldown: 0,
            thinking_budget: config.max_tokens,
            last_input_tokens: None,
            messages,
            build_baseline: None,
            consecutive_all_error_iterations: 0,
        }
    }

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn begin_iteration(&mut self, config: &AgentLoopConfig, iteration: usize) {
        self.build_cooldown = self.build_cooldown.saturating_sub(1);
        self.blocking_ctx.decrement_cooldowns();
        if iteration >= config.thinking_taper_after {
            self.thinking_budget =
                (f64::from(self.thinking_budget) * config.thinking_taper_factor) as u32;
            self.thinking_budget = self.thinking_budget.max(config.thinking_min_budget);
        }
    }

    fn build_request(
        &self,
        config: &AgentLoopConfig,
        tools: &[ToolDefinition],
        iteration: usize,
    ) -> ModelRequest {
        let (effective_tools, tool_choice) = match (&config.tool_hints, iteration) {
            (Some(hints), 0) if !hints.is_empty() => {
                let filtered: Vec<_> = tools
                    .iter()
                    .filter(|t| hints.iter().any(|h| h == &t.name))
                    .cloned()
                    .collect();
                if filtered.is_empty() {
                    (tools.to_vec(), aura_reasoner::ToolChoice::Auto)
                } else if filtered.len() == 1 {
                    let name = filtered[0].name.clone();
                    (filtered, aura_reasoner::ToolChoice::Tool { name })
                } else {
                    (filtered, aura_reasoner::ToolChoice::Required)
                }
            }
            _ => (tools.to_vec(), aura_reasoner::ToolChoice::Auto),
        };

        ModelRequest::builder(&config.model, &config.system_prompt)
            .messages(self.messages.clone())
            .tools(effective_tools)
            .tool_choice(tool_choice)
            .max_tokens(self.thinking_budget)
            .auth_token(config.auth_token.clone())
            .aura_project_id(config.aura_project_id.clone())
            .aura_agent_id(config.aura_agent_id.clone())
            .aura_session_id(config.aura_session_id.clone())
            .aura_org_id(config.aura_org_id.clone())
            .build()
    }
}

/// Run post-iteration checks (checkpoint, compaction, budget). Returns `true` to break.
fn post_iteration_checks(
    config: &AgentLoopConfig,
    event_tx: Option<&Sender<AgentLoopEvent>>,
    state: &mut LoopState,
    iteration: usize,
) -> bool {
    context::emit_checkpoint_if_needed(event_tx, state);
    context::compact_exploration_if_needed(config, state);
    context::check_budget_warnings(config, event_tx, state, iteration);
    if context::should_stop_for_budget(config, state, iteration) {
        state.result.timed_out = true;
        return true;
    }
    false
}

fn is_cancelled(token: Option<&CancellationToken>) -> bool {
    token.is_some_and(CancellationToken::is_cancelled)
}
