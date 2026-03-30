//! Turn Processor for Claude Code-like agentic loop.
//!
//! The Turn Processor handles multi-step conversations where the model
//! can request tools, receive results, and continue until completion.
//!
//! ## Turn Loop
//!
//! ```text
//! loop {
//!     1. Build context (deterministic)
//!     2. Call ModelProvider.complete()
//!     3. Record assistant response
//!     4. If tool_use: authorize → execute → inject tool_result
//!     5. If end_turn: finalize
//! }
//! ```
//!
//! ## Recording and Replay
//!
//! During normal operation, all model outputs and tool results are recorded.
//! During replay, the recorded data is used instead of calling the model/tools,
//! ensuring deterministic state reconstruction.

mod config;
mod delegate;
mod loop_runner;
mod record;
mod streaming;
mod tool_execution;
mod types;

pub use config::{StepConfig, TurnConfig};
pub use delegate::ModelCallDelegate;
pub use streaming::{StreamCallback, StreamCallbackEvent};
pub use types::{ExecutedToolCall, StepResult, ToolCache, TurnEntry, TurnResult};

use aura_kernel::{Policy, PolicyConfig};
use aura_core::{AgentId, Transaction};
use aura_kernel::ExecutorRouter;
use aura_reasoner::{
    Message, ModelProvider, ModelRequest, ModelResponse, StopReason, ToolDefinition,
    ToolResultContent,
};
use aura_store::Store;
use aura_tools::ToolRegistry;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument};

// ============================================================================
// Turn Processor
// ============================================================================

/// Turn processor for multi-step agentic conversations.
///
/// The Turn Processor implements the core agentic loop where the model
/// can propose tool uses, receive results, and continue until it decides
/// to end the turn.
///
/// ## Streaming
///
/// Set a streaming callback via `with_stream_callback()` to receive real-time
/// text updates as the model generates its response.
pub struct TurnProcessor<P, S, R>
where
    P: ModelProvider,
    S: Store,
    R: ToolRegistry,
{
    provider: Arc<P>,
    store: Arc<S>,
    executor: ExecutorRouter,
    policy: Policy,
    tool_registry: Arc<R>,
    config: TurnConfig,
    /// Optional callback for streaming text events
    stream_callback: Option<Arc<StreamCallback>>,
    /// Optional cancellation token to abort the turn loop.
    cancellation_token: Option<CancellationToken>,
}

impl<P, S, R> TurnProcessor<P, S, R>
where
    P: ModelProvider,
    S: Store,
    R: ToolRegistry,
{
    /// Create a new turn processor.
    #[must_use]
    pub fn new(
        provider: Arc<P>,
        store: Arc<S>,
        executor: ExecutorRouter,
        tool_registry: Arc<R>,
        config: TurnConfig,
    ) -> Self {
        let policy = Policy::new(PolicyConfig::default());
        Self {
            provider,
            store,
            executor,
            policy,
            tool_registry,
            config,
            stream_callback: None,
            cancellation_token: None,
        }
    }

    /// Create a turn processor with custom policy.
    #[must_use]
    pub fn with_policy(mut self, policy_config: PolicyConfig) -> Self {
        self.policy = Policy::new(policy_config);
        self
    }

    /// Set a callback for streaming text events.
    ///
    /// The callback will be invoked for each text delta received from the model,
    /// allowing real-time display of the response.
    #[must_use]
    pub fn with_stream_callback(mut self, callback: StreamCallback) -> Self {
        self.stream_callback = Some(Arc::new(callback));
        self
    }

    /// Set a callback for streaming text events (arc version).
    pub fn set_stream_callback(&mut self, callback: Arc<StreamCallback>) {
        self.stream_callback = Some(callback);
    }

    /// Clear the streaming callback.
    pub fn clear_stream_callback(&mut self) {
        self.stream_callback = None;
    }

    /// Set a cancellation token that can abort the turn loop.
    pub fn set_cancellation_token(&mut self, token: CancellationToken) {
        self.cancellation_token = Some(token);
    }

    /// Emit a streaming event to the callback (if set).
    pub(crate) fn emit_stream_event(&self, event: StreamCallbackEvent) {
        if let Some(callback) = &self.stream_callback {
            callback(event);
        }
    }

    /// Check whether the current turn has been cancelled.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancellation_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    }

    /// Get the workspace path for an agent.
    pub(crate) fn agent_workspace(&self, agent_id: &AgentId) -> PathBuf {
        self.config.workspace_base.join(agent_id.to_hex())
    }

    /// Build tool definitions from the registry.
    fn build_tools(&self) -> Vec<ToolDefinition> {
        self.tool_registry.list()
    }

    /// Process a user transaction through the full turn loop.
    ///
    /// This is the main entry point for processing a user message when
    /// conversation history is loaded from the store. For WebSocket sessions
    /// that maintain their own message history, use [`process_turn_with_messages`].
    ///
    /// # Errors
    ///
    /// Returns error if model completion or tool execution fails.
    #[instrument(skip(self, tx), fields(agent_id = %agent_id, hash = %tx.hash))]
    pub async fn process_turn(
        &self,
        agent_id: AgentId,
        tx: Transaction,
        next_seq: u64,
    ) -> Result<TurnResult, crate::runtime::RuntimeError> {
        info!("Starting turn processing (store-based history)");
        let messages = self.build_initial_messages(agent_id, &tx, next_seq);
        self.run_turn_loop(messages, agent_id).await
            .map_err(|e| crate::runtime::RuntimeError::Internal(e.to_string()))
    }

    /// Process a turn with pre-built message history.
    ///
    /// Unlike [`process_turn`], this method does not load history from the
    /// store. The caller is responsible for providing the full conversation
    /// context (including the current user message) in `messages`.
    ///
    /// This is the primary entry point for WebSocket sessions that maintain
    /// their own `Vec<Message>` across turns.
    ///
    /// # Errors
    ///
    /// Returns error if model completion or tool execution fails.
    #[instrument(skip(self, messages), fields(agent_id = %agent_id))]
    pub async fn process_turn_with_messages(
        &self,
        agent_id: AgentId,
        messages: Vec<Message>,
    ) -> Result<TurnResult, crate::runtime::RuntimeError> {
        info!("Starting turn processing (caller-provided history)");
        self.run_turn_loop(messages, agent_id).await
            .map_err(|e| crate::runtime::RuntimeError::Internal(e.to_string()))
    }

    /// Make a model call through the turn processor's infrastructure.
    ///
    /// Uses the same streaming, cancellation, and error handling as
    /// `process_step()`, but does **not** build its own request or execute
    /// tools. The caller provides a pre-built [`ModelRequest`] and receives
    /// the raw [`ModelResponse`].
    ///
    /// Designed for external orchestrators (e.g., `AgentLoop`) that manage
    /// their own tool execution and request building, but want to leverage
    /// `TurnProcessor`'s streaming, cancellation, and replay infrastructure.
    ///
    /// # Errors
    ///
    /// Returns error if the model completion fails.
    pub async fn resolve_model_call(&self, request: ModelRequest) -> Result<ModelResponse, crate::runtime::RuntimeError> {
        self.resolve_model_response(request).await
            .map_err(|e| crate::runtime::RuntimeError::Model(e.to_string()))
    }

    /// Resolve a model response: replay stub, streaming completion, or
    /// standard completion depending on configuration.
    async fn resolve_model_response(&self, request: ModelRequest) -> anyhow::Result<ModelResponse> {
        if self.config.replay_mode {
            debug!("Replay mode: skipping model call");
            Ok(ModelResponse::new(
                StopReason::EndTurn,
                Message::assistant("(replay)"),
                aura_reasoner::Usage::default(),
                aura_reasoner::ProviderTrace::new("replay", 0),
            ))
        } else if self.stream_callback.is_some() {
            self.complete_with_streaming(request).await
        } else {
            self.provider.complete(request).await.map_err(Into::into)
        }
    }

    /// Emit `ToolComplete` streaming events for each executed tool call.
    fn emit_tool_completions(&self, executed_tools: &[ExecutedToolCall]) {
        for tool in executed_tools {
            let result_text = match &tool.result {
                ToolResultContent::Text(s) => s.clone(),
                ToolResultContent::Json(v) => serde_json::to_string(v).unwrap_or_default(),
            };
            self.emit_stream_event(StreamCallbackEvent::ToolComplete {
                name: tool.tool_name.clone(),
                args: tool.tool_args.clone(),
                result: result_text,
                is_error: tool.is_error,
            });
        }
    }

    /// Process a single step: one model call, optional tool execution, and result.
    ///
    /// This is the atomic unit of the agentic loop. The caller is responsible for:
    /// - Managing the message history
    /// - Deciding whether to continue looping
    /// - Context truncation/compaction
    /// - Token budget tracking
    ///
    /// # Errors
    ///
    /// Returns error if model completion or tool execution fails.
    pub async fn process_step(
        &self,
        messages: &[Message],
        agent_id: AgentId,
        tool_cache: &mut ToolCache,
        step_config: &StepConfig,
    ) -> Result<StepResult, crate::runtime::RuntimeError> {
        let tools = self.build_tools();
        let model = step_config
            .model_override
            .as_deref()
            .unwrap_or(&self.config.model);

        let request = ModelRequest::builder(model, &self.config.system_prompt)
            .messages(messages.to_vec())
            .tools(tools)
            .max_tokens(self.config.max_tokens)
            .temperature(self.config.temperature.unwrap_or(0.2))
            .build();

        let response = self.resolve_model_response(request).await
            .map_err(|e| crate::runtime::RuntimeError::Model(e.to_string()))?;

        debug!(
            stop_reason = ?response.stop_reason,
            input_tokens = response.usage.input_tokens,
            output_tokens = response.usage.output_tokens,
            "Received model response"
        );

        match response.stop_reason {
            StopReason::ToolUse => {
                let executed_tools = self
                    .execute_tool_calls(&response.message, agent_id, tool_cache)
                    .await
                    .map_err(|e| crate::runtime::RuntimeError::ToolExecution(e.to_string()))?;
                let had_failures = executed_tools.iter().any(|t| t.is_error);
                self.emit_tool_completions(&executed_tools);
                Ok(StepResult {
                    response,
                    executed_tools,
                    stop_reason: StopReason::ToolUse,
                    had_failures,
                })
            }
            stop_reason @ (StopReason::EndTurn
            | StopReason::MaxTokens
            | StopReason::StopSequence) => Ok(StepResult {
                response,
                executed_tools: vec![],
                stop_reason,
                had_failures: false,
            }),
        }
    }
}

#[cfg(test)]
mod tests;
