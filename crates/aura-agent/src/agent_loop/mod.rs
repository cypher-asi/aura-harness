//! Main agent loop orchestrator.
//!
//! `AgentLoop` drives the multi-step agentic conversation by calling
//! the model provider in a loop with intelligence: blocking detection,
//! compaction, sanitization, budget management, etc.

mod context;
mod iteration;
mod search_cache;
mod streaming;
mod tool_execution;
#[cfg(test)]
mod tool_execution_tests;
mod tool_processing;

#[cfg(test)]
mod contract_tests;
#[cfg(test)]
mod cutover_tests;
#[cfg(test)]
mod parity_tests;
#[cfg(test)]
mod pipeline_tests;
#[cfg(test)]
mod streaming_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_advanced;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aura_reasoner::{Message, ModelProvider, ModelRequest, Role, StopReason, ToolDefinition};
use aura_tools::IntentClassifier;
use chrono::Utc;
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
use crate::events::{AgentLoopEvent, DebugEvent};
use crate::read_guard::ReadGuardState;
use crate::types::{AgentLoopResult, AgentToolExecutor, BuildBaseline, TurnObserver};

/// Configuration for the agent loop.
#[derive(Clone)]
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
    /// Post-turn observers (e.g. memory ingestion).
    /// Called automatically at the end of every turn inside the loop.
    pub observers: Vec<Arc<dyn TurnObserver>>,
    /// Optional keyword-driven intent classifier used to narrow the
    /// per-turn visible tool set based on the latest user message.
    ///
    /// Ships with an accompanying [`intent_classifier_manifest`] that
    /// maps tool names to their snake-case domain. Tools not present in
    /// the manifest are passed through unchanged, so core filesystem /
    /// shell tools stay visible regardless of classifier state.
    ///
    /// Populated via [`aura_protocol::SessionInit::intent_classifier`]
    /// (see `aura-os-super-agent::harness_handoff`) to let the harness
    /// reproduce the aura-os CEO super-agent's tier-1/tier-2 filtering
    /// without baking the tool manifest into the harness binary.
    ///
    /// [`intent_classifier_manifest`]: Self::intent_classifier_manifest
    pub intent_classifier: Option<Arc<IntentClassifier>>,
    /// `(tool_name, domain)` pairs consumed by [`intent_classifier`].
    ///
    /// Empty when [`intent_classifier`] is `None`.
    ///
    /// [`intent_classifier`]: Self::intent_classifier
    pub intent_classifier_manifest: Vec<(String, String)>,
}

impl std::fmt::Debug for AgentLoopConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentLoopConfig")
            .field("max_iterations", &self.max_iterations)
            .field("model", &self.model)
            .field("observers", &self.observers.len())
            .finish_non_exhaustive()
    }
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
            observers: Vec::new(),
            intent_classifier: None,
            intent_classifier_manifest: Vec::new(),
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

    /// Update the auth token for subsequent model requests.
    pub fn set_auth_token(&mut self, token: Option<String>) {
        self.config.auth_token = token;
    }

    /// Get a mutable reference to the config for external injection.
    pub fn config_mut(&mut self) -> &mut AgentLoopConfig {
        &mut self.config
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
        // Route provider-level `debug.retry` observations back into the
        // `event_tx` channel by installing a task-local observer for
        // the duration of this turn. The observer forwards through the
        // same channel as UI events so downstream consumers see all
        // `debug.*` frames inline with the streaming text.
        let observer: Option<aura_reasoner::RetryObserver> = event_tx.as_ref().map(|tx| {
            let tx = tx.clone();
            Arc::new(move |info: aura_reasoner::RetryInfo| {
                let event = AgentLoopEvent::Debug(DebugEvent::Retry {
                    timestamp: Utc::now(),
                    reason: info.reason,
                    attempt: info.attempt,
                    wait_ms: info.wait_ms,
                    provider: Some(info.provider),
                    model: Some(info.model),
                    task_id: None,
                });
                if let Err(e) = tx.try_send(event) {
                    tracing::warn!("debug.retry channel full or closed: {e}");
                }
            }) as aura_reasoner::RetryObserver
        });

        let fut = self.run_inner(
            provider,
            executor,
            messages,
            tools,
            event_tx,
            cancellation_token,
        );
        match observer {
            Some(obs) => aura_reasoner::DEBUG_RETRY_OBSERVER.scope(obs, fut).await,
            None => fut.await,
        }
    }

    async fn run_inner(
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
            let iteration_started_at = Instant::now();
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
                Err(iteration::LlmCallError::PromptTooLong(msg)) => {
                    match self
                        .retry_after_context_overflow(
                            provider,
                            &tools,
                            iteration,
                            event_tx.as_ref(),
                            cancellation_token.as_ref(),
                            &mut state,
                            msg,
                        )
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            e.apply(&mut state.result, event_tx.as_ref());
                            break;
                        }
                    }
                }
                Err(e) => {
                    e.apply(&mut state.result, event_tx.as_ref());
                    break;
                }
            };

            iteration::accumulate_response(&mut state, &response);
            state.result.iterations = iteration + 1;
            streaming::emit_iteration_complete(
                event_tx.as_ref(),
                iteration,
                &response,
                iteration_started_at,
            );

            if self
                .dispatch_stop_reason(&response, executor, event_tx.as_ref(), &mut state)
                .await
            {
                break;
            }
            if iteration::update_narration_budget(event_tx.as_ref(), &mut state, &response) {
                break;
            }
            if post_iteration_checks(&self.config, event_tx.as_ref(), &mut state, iteration) {
                break;
            }
        }

        state.result.messages = state.messages;

        for observer in &self.config.observers {
            observer.on_turn_complete(&state.result).await;
        }

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

    #[allow(clippy::too_many_arguments)] // TODO(W3): regroup retry inputs behind a `RetryCtx` struct.
    async fn retry_after_context_overflow(
        &self,
        provider: &dyn ModelProvider,
        tools: &[ToolDefinition],
        iteration: usize,
        event_tx: Option<&Sender<AgentLoopEvent>>,
        cancellation_token: Option<&CancellationToken>,
        state: &mut LoopState,
        initial_error: String,
    ) -> Result<aura_reasoner::ModelResponse, iteration::LlmCallError> {
        let recovery_steps = [
            (
                crate::compaction::CompactionConfig::aggressive(),
                "Context limit reached; compacting older context, trimming response budget, and retrying.",
            ),
            (
                crate::compaction::CompactionConfig::micro(),
                "Context is still too large; applying emergency compaction, trimming response budget again, and retrying.",
            ),
        ];
        let mut last_error = initial_error;

        for (tier, warning) in recovery_steps {
            if !context::compact_for_overflow(state, tier) {
                debug!("Skipping overflow retry because compaction made no progress");
                continue;
            }

            state.thinking_budget =
                (state.thinking_budget / 2).max(self.config.thinking_min_budget);
            streaming::emit(event_tx, AgentLoopEvent::Warning(warning.to_string()));

            let request = state.build_request(&self.config, tools, iteration);
            match self
                .call_model(provider, request, event_tx, cancellation_token)
                .await
            {
                Ok(response) => return Ok(response),
                Err(iteration::LlmCallError::PromptTooLong(msg)) => {
                    last_error = msg;
                }
                Err(other) => return Err(other),
            }
        }

        Err(iteration::LlmCallError::PromptTooLong(last_error))
    }
}

/// Mutable state carried across iterations of the agent loop.
pub struct LoopState {
    pub(crate) result: AgentLoopResult,
    pub(crate) tool_cache: HashMap<String, String>,
    /// Secondary, normalized index for `search_code` / `find_files`
    /// that collapses alternation-order and trivial whitespace
    /// variants. Populated alongside `tool_cache` in `update_cache`;
    /// consulted only on a miss of the exact key. Cleared together
    /// with `tool_cache` on any successful write so the "write
    /// invalidates cache" invariant is preserved.
    pub(crate) fuzzy_tool_cache: HashMap<String, String>,
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
    pub(crate) last_context_tokens_estimate: Option<u64>,
    pub(crate) messages: Vec<Message>,
    pub(crate) build_baseline: Option<BuildBaseline>,
    /// Consecutive iterations where every tool call returned an error.
    pub(crate) consecutive_all_error_iterations: usize,
    /// Rolling count of output tokens produced across turns that
    /// emitted no `tool_use` blocks. Reset to zero whenever a turn
    /// executes at least one tool call or when the soft narration
    /// budget fires a steering injection.
    pub(crate) consecutive_narration_tokens: usize,
    /// Whether the most recently processed turn produced at least one
    /// `tool_use` block. Initialized to `true` so the first turn starts
    /// with a budget-clean state.
    pub(crate) last_turn_had_tool_call: bool,
}

impl LoopState {
    fn new(config: &AgentLoopConfig, messages: Vec<Message>) -> Self {
        Self {
            result: AgentLoopResult::default(),
            tool_cache: HashMap::new(),
            fuzzy_tool_cache: HashMap::new(),
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
            last_context_tokens_estimate: None,
            messages,
            build_baseline: None,
            consecutive_all_error_iterations: 0,
            consecutive_narration_tokens: 0,
            last_turn_had_tool_call: true,
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
        // Phase 3: narrow `tools` down to domain-relevant entries before the
        // tool-hints logic runs. The classifier is keyed on the most recent
        // pure-text user message, so scratchpad tool-result turns reuse the
        // previous filter rather than widening the surface back to every tool.
        let classifier_filtered: Vec<ToolDefinition> = match (
            config.intent_classifier.as_deref(),
            latest_user_text(&self.messages),
        ) {
            (Some(classifier), Some(text)) if !config.intent_classifier_manifest.is_empty() => {
                classifier.filter_tools(text, &config.intent_classifier_manifest, tools)
            }
            _ => tools.to_vec(),
        };

        let (effective_tools, tool_choice) = match (&config.tool_hints, iteration) {
            (Some(hints), 0) if !hints.is_empty() => {
                let filtered: Vec<_> = classifier_filtered
                    .iter()
                    .filter(|t| hints.iter().any(|h| h == &t.name))
                    .cloned()
                    .collect();
                if filtered.is_empty() {
                    (classifier_filtered, aura_reasoner::ToolChoice::Auto)
                } else if filtered.len() == 1 {
                    let name = filtered[0].name.clone();
                    (filtered, aura_reasoner::ToolChoice::Tool { name })
                } else {
                    (filtered, aura_reasoner::ToolChoice::Required)
                }
            }
            _ => (classifier_filtered, aura_reasoner::ToolChoice::Auto),
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

/// Return the text of the most recent user-role message whose content is
/// plain text (skipping tool-result turns, which carry tool output rather
/// than a natural-language intent).
///
/// Used by [`LoopState::build_request`] to feed the intent classifier on
/// every iteration — including scratchpad iterations that follow a tool
/// call — so the tool filter stays keyed on the original user intent
/// until the user speaks again.
fn latest_user_text(messages: &[Message]) -> Option<&str> {
    for msg in messages.iter().rev() {
        if matches!(msg.role, Role::User)
            && msg
                .content
                .iter()
                .any(|b| matches!(b, aura_reasoner::ContentBlock::Text { .. }))
        {
            return msg.content.iter().find_map(|b| match b {
                aura_reasoner::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            });
        }
    }
    None
}

#[cfg(test)]
mod intent_classifier_tests {
    use super::*;
    use aura_reasoner::ToolDefinition;
    use serde_json::json;
    use std::sync::Arc;

    fn mk_tool(name: &str) -> ToolDefinition {
        ToolDefinition::new(name, name, json!({}))
    }

    fn mk_config_with_classifier() -> AgentLoopConfig {
        let classifier = IntentClassifier::from_rules(
            vec!["project".to_string()],
            vec![("billing".to_string(), vec!["credit".to_string()])],
        );
        AgentLoopConfig {
            intent_classifier: Some(Arc::new(classifier)),
            intent_classifier_manifest: vec![
                ("create_project".to_string(), "project".to_string()),
                ("list_credits".to_string(), "billing".to_string()),
            ],
            ..AgentLoopConfig::default()
        }
    }

    #[test]
    fn build_request_filters_tier2_tools_when_not_triggered() {
        let config = mk_config_with_classifier();
        let state = LoopState::new(&config, vec![Message::user("hello there")]);
        let tools = vec![
            mk_tool("create_project"),
            mk_tool("list_credits"),
            mk_tool("read_file"),
        ];

        let req = state.build_request(&config, &tools, 1);
        let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();

        assert!(names.contains(&"create_project"), "tier-1 tool kept");
        assert!(names.contains(&"read_file"), "unmapped tool passes through");
        assert!(
            !names.contains(&"list_credits"),
            "tier-2 billing tool hidden"
        );
    }

    #[test]
    fn build_request_admits_tier2_when_keyword_matches() {
        let config = mk_config_with_classifier();
        let state = LoopState::new(
            &config,
            vec![Message::user("check my credit balance please")],
        );
        let tools = vec![mk_tool("create_project"), mk_tool("list_credits")];

        let req = state.build_request(&config, &tools, 1);
        let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"list_credits"));
        assert!(names.contains(&"create_project"));
    }

    #[test]
    fn build_request_skips_tool_result_messages_when_picking_intent() {
        let config = mk_config_with_classifier();
        let msgs = vec![
            Message::user("check my credit balance"),
            Message::assistant("calling tool"),
            Message::tool_results(vec![(
                "tu_1".into(),
                aura_reasoner::ToolResultContent::Text("100".into()),
                false,
            )]),
        ];
        let state = LoopState::new(&config, msgs);
        let tools = vec![mk_tool("list_credits"), mk_tool("create_project")];

        let req = state.build_request(&config, &tools, 2);
        let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"list_credits"),
            "classifier should still see original user message after a tool-result turn"
        );
    }

    #[test]
    fn build_request_passthrough_when_classifier_absent() {
        let config = AgentLoopConfig::default();
        let state = LoopState::new(&config, vec![Message::user("anything")]);
        let tools = vec![mk_tool("anything_tool")];
        let req = state.build_request(&config, &tools, 1);
        assert_eq!(req.tools.len(), 1);
    }
}
