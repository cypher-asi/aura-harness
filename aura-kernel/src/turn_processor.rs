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

use crate::policy::{PermissionLevel, Policy, PolicyConfig};
use aura_core::{
    Action, AgentId, Decision, Effect, EffectKind, EffectStatus, ProposalSet,
    RecordEntry, ToolCall, ToolResult, Transaction,
};
use aura_executor::{ExecuteContext, ExecutorRouter};
use aura_reasoner::{
    ContentBlock, Message, ModelProvider, ModelRequest, ModelResponse, StopReason,
    StreamAccumulator, StreamEvent, ToolDefinition, ToolResultContent,
};
use futures_util::StreamExt;
use aura_store::Store;
use aura_tools::ToolRegistry;
use bytes::Bytes;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, error, info, instrument, warn};

// ============================================================================
// Configuration
// ============================================================================

/// Turn processor configuration.
#[derive(Debug, Clone)]
pub struct TurnConfig {
    /// Maximum steps (model calls) per turn
    pub max_steps: u32,
    /// Maximum tool calls per step
    pub max_tool_calls_per_step: u32,
    /// Model timeout in milliseconds
    pub model_timeout_ms: u64,
    /// Tool execution timeout in milliseconds
    pub tool_timeout_ms: u64,
    /// Context window size (record entries)
    pub context_window: usize,
    /// Model to use
    pub model: String,
    /// System prompt
    pub system_prompt: String,
    /// Base workspace directory
    pub workspace_base: PathBuf,
    /// Whether we're in replay mode (skip model/tools)
    pub replay_mode: bool,
    /// Temperature for model calls
    pub temperature: Option<f32>,
    /// Max tokens per response
    pub max_tokens: u32,
}

impl Default for TurnConfig {
    fn default() -> Self {
        Self {
            max_steps: 25,
            max_tool_calls_per_step: 8,
            model_timeout_ms: 60_000,
            tool_timeout_ms: 30_000,
            context_window: 50,
            model: "claude-opus-4-5-20251101".to_string(),
            system_prompt: default_system_prompt(),
            workspace_base: PathBuf::from("./workspaces"),
            replay_mode: false,
            temperature: Some(0.7),
            max_tokens: 4096,
        }
    }
}

/// Default system prompt for the agent.
fn default_system_prompt() -> String {
    r"You are AURA, an autonomous AI coding assistant with FULL access to a real filesystem and command execution environment.

## Your Environment

You are running inside the AURA runtime which provides you with REAL tool execution capabilities. When you invoke a tool, it WILL be executed on the actual system and you WILL receive real results. This is NOT a simulation.

## Available Tools

You have access to the following tools that execute in the user's workspace:

### Filesystem Tools
- `fs_ls`: List directory contents - returns files, directories, sizes
- `fs_read`: Read file contents - use this to examine source code, configs, etc.
- `fs_stat`: Get file/directory metadata (size, type, permissions)
- `fs_write`: Write content to a file (creates or overwrites)
- `fs_edit`: Edit an existing file by replacing specific text

### Search Tools
- `search_code`: Search for patterns in code using regex across files

### Command Tools
- `cmd_run`: Execute shell commands (may require approval for certain commands)

## How to Work

1. **Explore First**: Use `fs_ls` and `fs_read` to understand the codebase structure
2. **Search When Needed**: Use `search_code` to find patterns, definitions, and usages
3. **Make Targeted Changes**: Use `fs_edit` for modifications, `fs_write` for new files
4. **Run Commands**: Use `cmd_run` to execute build tools, tests, git, etc.

## Important Guidelines

- All file paths are relative to the workspace root unless absolute
- You CAN and SHOULD use these tools to complete the user's requests
- When you request a tool, it will be executed and you'll receive the real output
- Be thoughtful about file modifications - prefer small, focused changes
- Explain your reasoning before making changes

You are fully capable of reading, modifying, and creating files, as well as running commands. Use your tools proactively to help the user.
"
    .to_string()
}

// ============================================================================
// Turn Result
// ============================================================================

/// Result of processing a turn.
#[derive(Debug)]
pub struct TurnResult {
    /// Record entries created during the turn
    pub entries: Vec<TurnEntry>,
    /// Final assistant message
    pub final_message: Option<Message>,
    /// Total tokens used
    pub total_input_tokens: u32,
    /// Total output tokens
    pub total_output_tokens: u32,
    /// Number of steps taken
    pub steps: u32,
    /// Whether any tools failed
    pub had_failures: bool,
}

/// Callback type for streaming text events.
///
/// This callback is invoked whenever a text delta is received from the model,
/// allowing real-time display of the response as it's generated.
pub type StreamCallback = Box<dyn Fn(StreamCallbackEvent) + Send + Sync>;

/// Events that can be sent via the streaming callback.
#[derive(Debug, Clone)]
pub enum StreamCallbackEvent {
    /// A chunk of thinking content was received
    ThinkingDelta(String),
    /// Thinking block completed
    ThinkingComplete,
    /// A chunk of text was received
    TextDelta(String),
    /// A tool use started
    ToolStart {
        /// Tool use ID
        id: String,
        /// Tool name
        name: String,
    },
    /// A tool use completed
    ToolComplete {
        /// Tool name
        name: String,
        /// Tool arguments (JSON)
        args: serde_json::Value,
        /// Tool result text
        result: String,
        /// Whether the tool failed
        is_error: bool,
    },
    /// Streaming is complete for this step
    StepComplete,
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

    /// Emit a streaming event to the callback (if set).
    fn emit_stream_event(&self, event: StreamCallbackEvent) {
        if let Some(callback) = &self.stream_callback {
            callback(event);
        }
    }

    /// Get the workspace path for an agent.
    fn agent_workspace(&self, agent_id: &AgentId) -> PathBuf {
        self.config.workspace_base.join(agent_id.to_hex())
    }

    /// Build tool definitions from the registry.
    fn build_tools(&self) -> Vec<ToolDefinition> {
        self.tool_registry.list()
    }

    /// Build initial messages including conversation history from the store.
    ///
    /// Loads up to `context_window` previous entries and converts them to messages,
    /// then appends the current user prompt. Stops at any `SessionStart` transaction
    /// to respect context boundaries.
    fn build_initial_messages(&self, agent_id: AgentId, tx: &Transaction, current_seq: u64) -> Vec<Message> {
        let mut messages = Vec::new();

        // Load conversation history from store
        if current_seq > 1 && self.config.context_window > 0 {
            // Calculate how far back to scan (at most context_window entries, starting from seq 1)
            let start_seq = current_seq.saturating_sub(self.config.context_window as u64).max(1);
            let limit = self.config.context_window;

            debug!(
                agent_id = %agent_id,
                start_seq = start_seq,
                limit = limit,
                "Loading conversation history"
            );

            if let Ok(entries) = self.store.scan_record(agent_id, start_seq, limit) {
                // Find the most recent SessionStart to determine context boundary
                let session_start_idx = entries
                    .iter()
                    .rposition(|e| e.tx.tx_type == aura_core::TransactionType::SessionStart);

                // Only process entries after the most recent SessionStart (if any)
                let relevant_entries = session_start_idx.map_or_else(
                    || &entries[..],
                    |idx| {
                        debug!(session_start_seq = entries[idx].seq, "Found session boundary");
                        &entries[idx + 1..]
                    },
                );

                for entry in relevant_entries {
                    // Convert each record entry to messages
                    // UserPrompt transactions become user messages
                    // AgentMsg transactions become assistant messages
                    match entry.tx.tx_type {
                        aura_core::TransactionType::UserPrompt => {
                            let content = String::from_utf8_lossy(&entry.tx.payload);
                            if !content.is_empty() {
                                messages.push(Message::user(content.to_string()));
                            }
                        }
                        aura_core::TransactionType::AgentMsg => {
                            let content = String::from_utf8_lossy(&entry.tx.payload);
                            if !content.is_empty() {
                                messages.push(Message::assistant(content.to_string()));
                            }
                        }
                        _ => {
                            // Skip other transaction types (System, ActionResult, Trigger, SessionStart)
                        }
                    }
                }
                debug!(
                    loaded_messages = messages.len(),
                    "Loaded conversation history"
                );
            }
        }

        // Append current user prompt
        let prompt = String::from_utf8_lossy(&tx.payload);
        debug!(
            current_prompt = %prompt,
            history_count = messages.len(),
            "Building messages for model"
        );
        messages.push(Message::user(prompt.to_string()));

        // Log all messages being sent
        for (i, msg) in messages.iter().enumerate() {
            let content_preview: String = msg.text_content().chars().take(50).collect();
            debug!(
                idx = i,
                role = ?msg.role,
                content_preview = %content_preview,
                "Message in context"
            );
        }

        messages
    }

    /// Process a user transaction through the full turn loop.
    ///
    /// This is the main entry point for processing a user message.
    /// It loops until the model ends its turn or max steps is reached.
    ///
    /// # Errors
    ///
    /// Returns error if model completion or tool execution fails.
    #[allow(clippy::too_many_lines)] // Core orchestration logic - refactoring would hurt readability
    #[instrument(skip(self, tx), fields(agent_id = %agent_id, hash = %tx.hash))]
    pub async fn process_turn(
        &self,
        agent_id: AgentId,
        tx: Transaction,
        next_seq: u64,
    ) -> anyhow::Result<TurnResult> {
        info!("Starting turn processing");

        let mut entries = Vec::new();
        let mut messages = self.build_initial_messages(agent_id, &tx, next_seq);
        let tools = self.build_tools();

        let mut total_input_tokens = 0u32;
        let mut total_output_tokens = 0u32;
        let mut had_failures = false;
        let mut final_message = None;

        for step in 0..self.config.max_steps {
            debug!(step = step, messages = messages.len(), "Processing step");

            // 1. Build model request
            let request = ModelRequest::builder(&self.config.model, &self.config.system_prompt)
                .messages(messages.clone())
                .tools(tools.clone())
                .max_tokens(self.config.max_tokens)
                .temperature(self.config.temperature.unwrap_or(0.7))
                .build();

            // 2. Call model (skip in replay mode)
            let response = if self.config.replay_mode {
                debug!("Replay mode: skipping model call");
                // In replay mode, we would load from recorded entries
                // For now, return an empty end-turn response
                ModelResponse::new(
                    StopReason::EndTurn,
                    Message::assistant("(replay)"),
                    aura_reasoner::Usage::default(),
                    aura_reasoner::ProviderTrace::new("replay", 0),
                )
            } else if self.stream_callback.is_some() {
                // Use streaming completion with callback
                self.complete_with_streaming(request).await?
            } else {
                // Use non-streaming completion
                self.provider.complete(request).await?
            };

            // Track usage
            total_input_tokens += response.usage.input_tokens;
            total_output_tokens += response.usage.output_tokens;

            debug!(
                stop_reason = ?response.stop_reason,
                input_tokens = response.usage.input_tokens,
                output_tokens = response.usage.output_tokens,
                "Received model response"
            );

            // 3. Add assistant message to conversation
            messages.push(response.message.clone());
            final_message = Some(response.message.clone());

            // 4. Check stop reason and handle accordingly
            match response.stop_reason {
                StopReason::EndTurn => {
                    // Turn complete
                    info!(step = step, "Turn completed (end_turn)");
                    entries.push(TurnEntry {
                        turn_step: step,
                        model_response: response,
                        tool_results: vec![],
                        executed_tools: vec![],
                        stop_reason: StopReason::EndTurn,
                    });
                    break;
                }
                StopReason::ToolUse => {
                    // Extract and execute tool calls
                    let executed_tools = self.execute_tool_calls(&response.message, agent_id).await?;

                    // Check for failures
                    if executed_tools.iter().any(|t| t.is_error) {
                        had_failures = true;
                    }

                    // Emit ToolComplete events for each executed tool
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

                    // Convert to legacy format for backwards compatibility
                    let tool_results: Vec<(String, ToolResultContent, bool)> = executed_tools
                        .iter()
                        .map(|t| (t.tool_use_id.clone(), t.result.clone(), t.is_error))
                        .collect();

                    // Record this step
                    entries.push(TurnEntry {
                        turn_step: step,
                        model_response: response,
                        tool_results: tool_results.clone(),
                        executed_tools,
                        stop_reason: StopReason::ToolUse,
                    });

                    // Add tool results to conversation for next iteration
                    if !tool_results.is_empty() {
                        messages.push(Message::tool_results(tool_results));
                    }
                }
                StopReason::MaxTokens => {
                    warn!(step = step, "Turn stopped due to max_tokens");
                    entries.push(TurnEntry {
                        turn_step: step,
                        model_response: response,
                        tool_results: vec![],
                        executed_tools: vec![],
                        stop_reason: StopReason::MaxTokens,
                    });
                    break;
                }
                StopReason::StopSequence => {
                    debug!(step = step, "Turn stopped at stop sequence");
                    entries.push(TurnEntry {
                        turn_step: step,
                        model_response: response,
                        tool_results: vec![],
                        executed_tools: vec![],
                        stop_reason: StopReason::StopSequence,
                    });
                    break;
                }
            }
        }

        #[allow(clippy::cast_possible_truncation)] // entries count is bounded by max_steps
        let steps = entries.len() as u32;

        info!(
            steps = steps,
            input_tokens = total_input_tokens,
            output_tokens = total_output_tokens,
            "Turn processing complete"
        );

        Ok(TurnResult {
            entries,
            final_message,
            total_input_tokens,
            total_output_tokens,
            steps,
            had_failures,
        })
    }

    /// Complete a model request with streaming, emitting events to the callback.
    async fn complete_with_streaming(&self, request: ModelRequest) -> anyhow::Result<ModelResponse> {
        let start = std::time::Instant::now();

        // Get the streaming response
        let mut stream = self.provider.complete_streaming(request).await?;

        // Accumulate the response while emitting text deltas
        let mut accumulator = StreamAccumulator::new();
        let input_tokens = 0u32;
        let mut in_thinking_block = false;

        while let Some(event_result) = stream.next().await {
            match event_result {
                Ok(event) => {
                    // Emit events to the callback
                    match &event {
                        StreamEvent::ContentBlockStart {
                            content_type: aura_reasoner::StreamContentType::Thinking,
                            ..
                        } => {
                            in_thinking_block = true;
                        }
                        StreamEvent::ContentBlockStart {
                            content_type: aura_reasoner::StreamContentType::ToolUse { id, name },
                            ..
                        } => {
                            // If we were in a thinking block, signal it's complete
                            if in_thinking_block {
                                self.emit_stream_event(StreamCallbackEvent::ThinkingComplete);
                                in_thinking_block = false;
                            }
                            self.emit_stream_event(StreamCallbackEvent::ToolStart {
                                id: id.clone(),
                                name: name.clone(),
                            });
                        }
                        StreamEvent::ContentBlockStart {
                            content_type: aura_reasoner::StreamContentType::Text,
                            ..
                        }
                        | StreamEvent::ContentBlockStop { .. } => {
                            // If we were in a thinking block, signal it's complete
                            if in_thinking_block {
                                self.emit_stream_event(StreamCallbackEvent::ThinkingComplete);
                                in_thinking_block = false;
                            }
                        }
                        StreamEvent::ThinkingDelta { thinking } => {
                            self.emit_stream_event(StreamCallbackEvent::ThinkingDelta(thinking.clone()));
                        }
                        StreamEvent::TextDelta { text } => {
                            self.emit_stream_event(StreamCallbackEvent::TextDelta(text.clone()));
                        }
                        StreamEvent::Error { message } => {
                            error!(error = %message, "Stream error from provider");
                            anyhow::bail!("Stream error: {message}");
                        }
                        _ => {}
                    }

                    // Process the event to build the final response
                    accumulator.process(&event);

                    // Check for terminal events
                    if matches!(event, StreamEvent::MessageStop) {
                        break;
                    }
                }
                Err(e) => {
                    error!(error = %e, "Stream error");
                    anyhow::bail!("Stream error: {e}");
                }
            }
        }

        // Signal step completion
        self.emit_stream_event(StreamCallbackEvent::StepComplete);

        let latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

        // Convert accumulated state to a ModelResponse
        accumulator.into_response(input_tokens, latency_ms)
    }

    /// Execute tool calls from a model message.
    async fn execute_tool_calls(
        &self,
        message: &Message,
        agent_id: AgentId,
    ) -> anyhow::Result<Vec<ExecutedToolCall>> {
        let mut results = Vec::new();
        let workspace = self.agent_workspace(&agent_id);

        // Ensure workspace exists
        if let Err(e) = tokio::fs::create_dir_all(&workspace).await {
            error!(error = %e, "Failed to create workspace");
        }

        for block in &message.content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                debug!(tool = %name, id = %id, "Executing tool");

                // 1. Check policy
                let permission = self.policy.check_tool_permission(name);

                match permission {
                    PermissionLevel::Deny => {
                        warn!(tool = %name, "Tool denied by policy");
                        results.push(ExecutedToolCall {
                            tool_use_id: id.clone(),
                            tool_name: name.clone(),
                            tool_args: input.clone(),
                            result: ToolResultContent::text(format!("Tool '{name}' is not allowed")),
                            is_error: true,
                        });
                        continue;
                    }
                    PermissionLevel::AlwaysAsk => {
                        // For now, treat AlwaysAsk as requiring approval
                        // The CLI will handle the approval flow
                        debug!(tool = %name, "Tool requires approval (AlwaysAsk)");
                    }
                    PermissionLevel::AskOnce => {
                        // For now, allow after first ask
                        debug!(tool = %name, "Tool allowed (AskOnce)");
                    }
                    PermissionLevel::AlwaysAllow => {
                        debug!(tool = %name, "Tool allowed (AlwaysAllow)");
                    }
                }

                // 2. Execute tool
                let tool_call = ToolCall::new(name.clone(), input.clone());
                let action = Action::delegate_tool(&tool_call);
                let ctx = ExecuteContext::new(agent_id, action.action_id, workspace.clone());

                match self.executor.execute(&ctx, &action).await {
                    effect if effect.status == EffectStatus::Committed => {
                        // Parse the tool result from the effect payload
                        if let Ok(tool_result) =
                            serde_json::from_slice::<ToolResult>(&effect.payload)
                        {
                            let content = if tool_result.stdout.is_empty() {
                                ToolResultContent::text("Success (no output)")
                            } else {
                                ToolResultContent::text(
                                    String::from_utf8_lossy(&tool_result.stdout).to_string(),
                                )
                            };
                            results.push(ExecutedToolCall {
                                tool_use_id: id.clone(),
                                tool_name: name.clone(),
                                tool_args: input.clone(),
                                result: content,
                                is_error: !tool_result.ok,
                            });
                        } else {
                            results.push(ExecutedToolCall {
                                tool_use_id: id.clone(),
                                tool_name: name.clone(),
                                tool_args: input.clone(),
                                result: ToolResultContent::text("Tool executed successfully"),
                                is_error: false,
                            });
                        }
                    }
                    effect => {
                        // Tool failed
                        let error_msg = if let Ok(tool_result) =
                            serde_json::from_slice::<ToolResult>(&effect.payload)
                        {
                            String::from_utf8_lossy(&tool_result.stderr).to_string()
                        } else {
                            "Tool execution failed".to_string()
                        };
                        results.push(ExecutedToolCall {
                            tool_use_id: id.clone(),
                            tool_name: name.clone(),
                            tool_args: input.clone(),
                            result: ToolResultContent::text(error_msg),
                            is_error: true,
                        });
                    }
                }
            }
        }

        Ok(results)
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
    ) -> RecordEntry {
        // Build proposals from the turn entries
        let proposals = ProposalSet::new();

        // Build decision
        let mut decision = Decision::new();

        // Build actions and effects from tool calls
        let mut actions = Vec::new();
        let mut effects = Vec::new();

        for entry in &turn_result.entries {
            for executed_tool in &entry.executed_tools {
                // Create a proper ToolCall with full information
                let tool_call = ToolCall::new(
                    executed_tool.tool_name.clone(),
                    executed_tool.tool_args.clone(),
                );

                // Create action using the delegate_tool helper which properly serializes
                let action = Action::delegate_tool(&tool_call);
                let action_id = action.action_id;
                actions.push(action);

                // Record acceptance in decision
                decision.accept(action_id);

                // Create effect
                let effect_status = if executed_tool.is_error {
                    EffectStatus::Failed
                } else {
                    EffectStatus::Committed
                };

                let payload = match &executed_tool.result {
                    ToolResultContent::Text(s) => Bytes::from(s.clone()),
                    ToolResultContent::Json(v) => {
                        Bytes::from(serde_json::to_vec(v).unwrap_or_default())
                    }
                };

                let effect = Effect::new(action_id, EffectKind::Agreement, effect_status, payload);
                effects.push(effect);
            }
        }

        RecordEntry::builder(seq, tx)
            .context_hash(context_hash)
            .proposals(proposals)
            .decision(decision)
            .actions(actions)
            .effects(effects)
            .build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_reasoner::{MockProvider, MockResponse};
    use aura_store::RocksStore;
    use aura_tools::DefaultToolRegistry;
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

        // Create a mock that first requests a tool, then ends
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

        // Should have 2 steps: tool use + end turn
        assert_eq!(result.steps, 2);
    }

    #[tokio::test]
    async fn test_max_steps_limit() {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();

        // Create a mock that always requests tools (never ends)
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
            max_steps: 3, // Limit to 3 steps
            ..TurnConfig::default()
        };

        let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

        let tx = Transaction::user_prompt(AgentId::generate(), "Keep using tools");
        let result = processor.process_turn(tx.agent_id, tx, 1).await.unwrap();

        // Should stop at max_steps
        assert_eq!(result.steps, 3);
    }
}
