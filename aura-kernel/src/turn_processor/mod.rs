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
mod loop_runner;
mod streaming;
mod tool_execution;

pub use config::{StepConfig, TurnConfig};
pub use streaming::{StreamCallback, StreamCallbackEvent};

use crate::policy::{Policy, PolicyConfig};
use aura_core::{
    Action, AgentId, AuraError, Decision, Effect, EffectKind, EffectStatus, ProposalSet,
    RecordEntry, ToolCall, Transaction,
};
use aura_executor::ExecutorRouter;
use aura_reasoner::{
    Message, ModelProvider, ModelRequest, ModelResponse, StopReason, ToolDefinition,
    ToolResultContent,
};
use aura_store::Store;
use aura_tools::ToolRegistry;
use bytes::Bytes;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument};

/// Per-turn cache for tool results.
///
/// Keyed by `"tool_name\0canonical_args_json"`. Only populated for
/// read-only tools (`fs_ls`, `fs_read`, `fs_stat`, `fs_find`, `search_code`)
/// to avoid suppressing side-effectful calls.
pub type ToolCache = HashMap<String, ExecutedToolCall>;

/// Result of processing a single step (one model call + tool execution).
#[derive(Debug)]
pub struct StepResult {
    /// The model's response for this step.
    pub response: ModelResponse,
    /// Tool calls that were executed during this step.
    pub executed_tools: Vec<ExecutedToolCall>,
    /// Why the model stopped generating.
    pub stop_reason: StopReason,
    /// Whether any tool executions failed.
    pub had_failures: bool,
}

/// Result of processing a turn.
#[derive(Debug)]
pub struct TurnResult {
    /// Record entries created during the turn
    pub entries: Vec<TurnEntry>,
    /// Final assistant message
    pub final_message: Option<Message>,
    /// Total tokens used
    pub total_input_tokens: u64,
    /// Total output tokens
    pub total_output_tokens: u64,
    /// Number of steps taken
    pub steps: u32,
    /// Whether any tools failed
    pub had_failures: bool,
    /// Whether the turn was cancelled.
    pub cancelled: bool,
    /// Model identifier used for this turn.
    pub model: String,
    /// Provider name (e.g., "anthropic").
    pub provider: String,
}

/// Information about an executed tool call.
#[derive(Debug, Clone)]
pub struct ExecutedToolCall {
    /// Tool use ID from the model
    pub tool_use_id: String,
    /// Tool name
    pub tool_name: String,
    /// Tool arguments (JSON)
    pub tool_args: serde_json::Value,
    /// Tool result
    pub result: ToolResultContent,
    /// Whether the tool failed
    pub is_error: bool,
    /// Metadata from the tool result (e.g. `file_existed`, `bytes_written`).
    pub metadata: std::collections::HashMap<String, String>,
}

/// A single step entry in a turn.
#[derive(Debug, Clone)]
pub struct TurnEntry {
    /// Step number within the turn (0-indexed)
    pub turn_step: u32,
    /// Model response for this step
    pub model_response: ModelResponse,
    /// Tool results from this step (if any) - legacy format for backwards compatibility
    pub tool_results: Vec<(String, ToolResultContent, bool)>,
    /// Executed tool calls with full information
    pub executed_tools: Vec<ExecutedToolCall>,
    /// Stop reason for this step
    pub stop_reason: StopReason,
}

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
    fn emit_stream_event(&self, event: StreamCallbackEvent) {
        if let Some(callback) = &self.stream_callback {
            callback(event);
        }
    }

    /// Check whether the current turn has been cancelled.
    fn is_cancelled(&self) -> bool {
        self.cancellation_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    }

    /// Get the workspace path for an agent.
    fn agent_workspace(&self, agent_id: &AgentId) -> PathBuf {
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
    ) -> anyhow::Result<TurnResult> {
        info!("Starting turn processing (store-based history)");
        let messages = self.build_initial_messages(agent_id, &tx, next_seq);
        self.run_turn_loop(messages, agent_id).await
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
    ) -> anyhow::Result<TurnResult> {
        info!("Starting turn processing (caller-provided history)");
        self.run_turn_loop(messages, agent_id).await
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
    ) -> anyhow::Result<StepResult> {
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

        let response = if self.config.replay_mode {
            debug!("Replay mode: skipping model call");
            ModelResponse::new(
                StopReason::EndTurn,
                Message::assistant("(replay)"),
                aura_reasoner::Usage::default(),
                aura_reasoner::ProviderTrace::new("replay", 0),
            )
        } else if self.stream_callback.is_some() {
            self.complete_with_streaming(request).await?
        } else {
            self.provider.complete(request).await?
        };

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
                    .await?;

                let had_failures = executed_tools.iter().any(|t| t.is_error);

                for tool in &executed_tools {
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

    /// Convert turn results to a `RecordEntry` for storage.
    ///
    /// This properly records all tool calls with their full information (tool name, args, results).
    pub fn to_record_entry(
        &self,
        seq: u64,
        tx: Transaction,
        turn_result: &TurnResult,
        context_hash: [u8; 32],
    ) -> Result<RecordEntry, AuraError> {
        let proposals = ProposalSet::new();
        let mut decision = Decision::new();
        let mut actions = Vec::new();
        let mut effects = Vec::new();

        for entry in &turn_result.entries {
            for executed_tool in &entry.executed_tools {
                let tool_call = ToolCall::new(
                    executed_tool.tool_name.clone(),
                    executed_tool.tool_args.clone(),
                );

                let action = Action::delegate_tool(&tool_call)?;
                let action_id = action.action_id;
                actions.push(action);

                decision.accept(action_id);

                let effect_status = if executed_tool.is_error {
                    EffectStatus::Failed
                } else {
                    EffectStatus::Committed
                };

                let payload = match &executed_tool.result {
                    ToolResultContent::Text(s) => Bytes::from(s.clone()),
                    ToolResultContent::Json(v) => Bytes::from(serde_json::to_vec(v)?),
                };

                let effect = Effect::new(action_id, EffectKind::Agreement, effect_status, payload);
                effects.push(effect);
            }
        }

        Ok(RecordEntry::builder(seq, tx)
            .context_hash(context_hash)
            .proposals(proposals)
            .decision(decision)
            .actions(actions)
            .effects(effects)
            .build())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_reasoner::{MockProvider, MockResponse};
    use aura_store::RocksStore;
    use aura_tools::DefaultToolRegistry;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_processor() -> (
        TurnProcessor<MockProvider, RocksStore, DefaultToolRegistry>,
        TempDir,
        TempDir,
    ) {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();

        let provider = Arc::new(MockProvider::simple_response("Hello!"));
        let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
        let executor = ExecutorRouter::new();
        let tool_registry = Arc::new(DefaultToolRegistry::new());

        let config = TurnConfig {
            workspace_base: ws_dir.path().to_path_buf(),
            ..TurnConfig::default()
        };

        let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);
        (processor, db_dir, ws_dir)
    }

    #[tokio::test]
    async fn test_simple_turn() {
        let (processor, _db_dir, _ws_dir) = create_test_processor();

        let tx = Transaction::user_prompt(AgentId::generate(), "Hello");
        let result = processor.process_turn(tx.agent_id, tx, 1).await.unwrap();

        assert_eq!(result.steps, 1);
        assert!(!result.had_failures);
        assert!(result.final_message.is_some());
    }

    #[tokio::test]
    async fn test_turn_with_tool_use() {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();

        let provider = Arc::new(
            MockProvider::new()
                .with_response(MockResponse::tool_use(
                    "tool_1",
                    "fs.ls",
                    serde_json::json!({ "path": "." }),
                ))
                .with_response(MockResponse::text("I listed the files.")),
        );

        let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
        let executor = ExecutorRouter::new();
        let tool_registry = Arc::new(DefaultToolRegistry::new());

        let config = TurnConfig {
            workspace_base: ws_dir.path().to_path_buf(),
            ..TurnConfig::default()
        };

        let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

        let tx = Transaction::user_prompt(AgentId::generate(), "List files");
        let result = processor.process_turn(tx.agent_id, tx, 1).await.unwrap();

        assert_eq!(result.steps, 2);
    }

    #[tokio::test]
    async fn test_max_steps_limit() {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();

        let provider = Arc::new(
            MockProvider::new().with_default_response(MockResponse::tool_use(
                "tool_1",
                "fs.ls",
                serde_json::json!({ "path": "." }),
            )),
        );

        let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
        let executor = ExecutorRouter::new();
        let tool_registry = Arc::new(DefaultToolRegistry::new());

        let config = TurnConfig {
            workspace_base: ws_dir.path().to_path_buf(),
            max_steps: 3,
            ..TurnConfig::default()
        };

        let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

        let tx = Transaction::user_prompt(AgentId::generate(), "Keep using tools");
        let result = processor.process_turn(tx.agent_id, tx, 1).await.unwrap();

        assert_eq!(result.steps, 3);
    }

    #[tokio::test]
    async fn test_process_step_returns_end_turn() {
        let (processor, _db_dir, _ws_dir) = create_test_processor();

        let messages = vec![Message::user("Hello")];
        let agent_id = AgentId::generate();
        let mut tool_cache: ToolCache = HashMap::new();

        let result = processor
            .process_step(&messages, agent_id, &mut tool_cache, &StepConfig::default())
            .await
            .unwrap();

        assert_eq!(result.stop_reason, StopReason::EndTurn);
        assert!(result.executed_tools.is_empty());
        assert!(!result.had_failures);
    }

    #[tokio::test]
    async fn test_process_step_returns_tool_use() {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();

        let provider = Arc::new(MockProvider::new().with_response(MockResponse::tool_use(
            "tool_1",
            "fs_ls",
            serde_json::json!({ "path": "." }),
        )));

        let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
        let executor = ExecutorRouter::new();
        let tool_registry = Arc::new(DefaultToolRegistry::new());

        let config = TurnConfig {
            workspace_base: ws_dir.path().to_path_buf(),
            ..TurnConfig::default()
        };

        let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

        let messages = vec![Message::user("List files")];
        let agent_id = AgentId::generate();
        let mut tool_cache: ToolCache = HashMap::new();

        let result = processor
            .process_step(&messages, agent_id, &mut tool_cache, &StepConfig::default())
            .await
            .unwrap();

        assert_eq!(result.stop_reason, StopReason::ToolUse);
        assert!(!result.executed_tools.is_empty());
    }

    #[tokio::test]
    async fn test_process_step_respects_model_override() {
        let (processor, _db_dir, _ws_dir) = create_test_processor();

        let messages = vec![Message::user("Hello")];
        let agent_id = AgentId::generate();
        let mut tool_cache: ToolCache = HashMap::new();

        let step_config = StepConfig {
            model_override: Some("override-model".to_string()),
            ..StepConfig::default()
        };

        let result = processor
            .process_step(&messages, agent_id, &mut tool_cache, &step_config)
            .await
            .unwrap();

        assert_eq!(result.stop_reason, StopReason::EndTurn);
        assert!(!result.had_failures);
    }

    #[tokio::test]
    async fn test_run_turn_loop_backward_compat() {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();

        let provider = Arc::new(
            MockProvider::new()
                .with_response(MockResponse::text("Hello from store!"))
                .with_response(MockResponse::text("Hello from messages!")),
        );

        let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
        let executor = ExecutorRouter::new();
        let tool_registry = Arc::new(DefaultToolRegistry::new());

        let config = TurnConfig {
            workspace_base: ws_dir.path().to_path_buf(),
            ..TurnConfig::default()
        };

        let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "Hello");
        let result_store = processor.process_turn(tx.agent_id, tx, 1).await.unwrap();

        let messages = vec![Message::user("Hello".to_string())];
        let result_msgs = processor
            .process_turn_with_messages(agent_id, messages)
            .await
            .unwrap();

        assert_eq!(result_store.steps, result_msgs.steps);
        assert_eq!(result_store.steps, 1);
        assert!(!result_store.had_failures);
        assert!(!result_msgs.had_failures);
    }

    #[test]
    fn test_step_config_default() {
        let config = StepConfig::default();
        assert!(config.thinking_budget.is_none());
        assert!(config.model_override.is_none());
        assert!(config.max_tool_calls.is_none());
    }
}
