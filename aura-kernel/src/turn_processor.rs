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
    Action, AgentId, Decision, Effect, EffectKind, EffectStatus, ProposalSet, RecordEntry,
    ToolCall, ToolResult, Transaction,
};
use aura_executor::{ExecuteContext, ExecutorRouter};
use aura_reasoner::{
    ContentBlock, Message, ModelProvider, ModelRequest, ModelResponse, StopReason,
    StreamAccumulator, StreamEvent, ToolDefinition, ToolResultContent,
};
use aura_store::Store;
use aura_tools::ToolRegistry;
use bytes::Bytes;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, warn};

/// Per-turn cache for tool results.
///
/// Keyed by `"tool_name\0canonical_args_json"`. Only populated for
/// read-only tools (`fs_ls`, `fs_read`, `fs_stat`, `fs_find`, `search_code`)
/// to avoid suppressing side-effectful calls.
pub type ToolCache = HashMap<String, ExecutedToolCall>;

/// Tools whose results are safe to cache (no side effects).
const CACHEABLE_TOOLS: &[&str] = &["fs_ls", "fs_read", "fs_stat", "fs_find", "search_code"];

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
    /// Context window size in tokens. When the estimated token count of
    /// `messages` exceeds `context_window_tokens * context_target_ratio`,
    /// older tool-result messages are truncated to stay within budget.
    pub context_window_tokens: usize,
    /// Target utilization ratio (0.0–1.0). Truncation triggers when
    /// estimated tokens exceed `context_window_tokens * context_target_ratio`.
    pub context_target_ratio: f32,
}

/// Approximate tokens-per-character ratio for English text. Claude
/// tokenisers average ~3.5–4 chars/token; we use 4 for a conservative
/// (over-)estimate that biases toward earlier truncation.
const CHARS_PER_TOKEN: usize = 4;

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
            temperature: Some(0.2),
            max_tokens: 16_384,
            context_window_tokens: 200_000,
            context_target_ratio: 0.80,
        }
    }
}

/// Per-step configuration overrides.
///
/// Enables the caller (e.g., `AgentLoop`) to adjust behavior on a per-step
/// basis — for example, tapering the thinking budget after early iterations.
#[derive(Debug, Clone, Default)]
pub struct StepConfig {
    /// Override the thinking budget for this step.
    pub thinking_budget: Option<u32>,
    /// Override the model for this step.
    pub model_override: Option<String>,
    /// Override the maximum tool calls for this step.
    pub max_tool_calls: Option<u32>,
}

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

/// Default system prompt for the agent.
fn default_system_prompt() -> String {
    r"You are AURA, an autonomous AI coding assistant with FULL access to a real filesystem and command execution environment.

## Your Environment

You are running inside the AURA runtime which provides you with REAL tool execution capabilities. When you invoke a tool, it WILL be executed on the actual system and you WILL receive real results. This is NOT a simulation.

## Available Tools

You have access to the following tools that execute in the user's workspace:

### Filesystem Tools
- `fs_ls`: List directory contents - returns files, directories, sizes
- `fs_read`: Read file contents (supports `start_line`/`end_line` for partial reads)
- `fs_stat`: Get file/directory metadata (size, type, permissions)
- `fs_write`: Write content to a file (creates or overwrites)
- `fs_edit`: Edit an existing file by replacing specific text

### Search Tools
- `search_code`: Search for patterns in code using regex across files

### Command Tools
- `cmd_run`: Execute shell commands (may require approval for certain commands)

## Planning and Execution Strategy

**Before writing any code or making changes, always:**

1. **Form a concrete plan**: State what you will do and in what order. Identify the files, functions, and changes needed before invoking any tools.
2. **Batch tool calls**: You can issue multiple tool calls in a single response (up to 8). Always batch independent reads, searches, and stat calls together rather than making them one at a time.
3. **Read selectively**: Use `start_line`/`end_line` on `fs_read` to fetch only the lines you need. Do NOT re-read files or sections already present in the conversation context.
4. **Track your progress**: After each step, briefly note what you accomplished and what remains.

## Efficiency Rules

- **Budget awareness**: You have a limited number of steps per turn. Converge toward a solution efficiently — do not explore aimlessly.
- **No redundant reads**: If a file's contents are already in context from a previous step, use that information instead of reading it again.
- **Prefer search over full reads**: Use `search_code` to locate relevant code before reading entire files.
- **Make targeted changes**: Use `fs_edit` for modifications, `fs_write` for new files. Prefer small, focused changes.
- **Verify once**: Run commands like `cargo check` or tests once after all edits are done, not after each individual edit.

## How to Work

1. **Plan**: Analyze the request and outline the steps you will take.
2. **Explore**: Use `fs_ls`, `search_code`, and selective `fs_read` to understand the relevant code.
3. **Implement**: Make all necessary changes using `fs_edit` and `fs_write`.
4. **Verify**: Run build/test commands to confirm correctness.

## Important Guidelines

- All file paths are relative to the workspace root unless absolute
- You CAN and SHOULD use these tools to complete the user's requests
- When you request a tool, it will be executed and you'll receive the real output
- Explain your reasoning briefly before making changes

You are fully capable of reading, modifying, and creating files, as well as running commands. Use your tools proactively and efficiently to help the user.
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
    /// Incremental snapshot of tool input JSON as it streams in.
    ToolInputSnapshot {
        /// Tool use ID
        id: String,
        /// Tool name
        name: String,
        /// Accumulated input JSON so far (may be partial/incomplete)
        input: String,
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
    /// An error occurred during the turn (LLM, tool callback, timeout, etc.).
    Error {
        /// Machine-readable error code.
        code: String,
        /// Human-readable description.
        message: String,
        /// Whether the session can continue after this error.
        recoverable: bool,
    },
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

    /// Build initial messages including conversation history from the store.
    ///
    /// Loads up to `context_window` previous entries and converts them to messages,
    /// then appends the current user prompt. Stops at any `SessionStart` transaction
    /// to respect context boundaries.
    fn build_initial_messages(
        &self,
        agent_id: AgentId,
        tx: &Transaction,
        current_seq: u64,
    ) -> Vec<Message> {
        let mut messages = Vec::new();

        // Load conversation history from store
        if current_seq > 1 && self.config.context_window > 0 {
            // Calculate how far back to scan (at most context_window entries, starting from seq 1)
            let start_seq = current_seq
                .saturating_sub(self.config.context_window as u64)
                .max(1);
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
                        debug!(
                            session_start_seq = entries[idx].seq,
                            "Found session boundary"
                        );
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

    /// Estimate the total character count of messages (used as a proxy for tokens).
    fn estimate_message_chars(messages: &[Message]) -> usize {
        messages.iter().map(|m| m.text_content().len()).sum()
    }

    /// Truncate the message list to fit within the context-window budget.
    ///
    /// Strategy: keep the first message (original user prompt) and the
    /// most-recent N messages. Middle messages — especially large
    /// tool-result messages — are dropped and replaced with a single
    /// "[truncated]" placeholder so the model knows context was trimmed.
    fn truncate_messages_if_needed(messages: &mut Vec<Message>, config: &TurnConfig) {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let budget_chars = (config.context_window_tokens as f64
            * f64::from(config.context_target_ratio)
            * CHARS_PER_TOKEN as f64) as usize;

        let total_chars = Self::estimate_message_chars(messages);

        if total_chars <= budget_chars || messages.len() <= 3 {
            return;
        }

        debug!(
            total_chars,
            budget_chars,
            message_count = messages.len(),
            "Context approaching limit — truncating older messages"
        );

        // Always keep: first message (user prompt) + last `keep_tail` messages.
        let keep_tail = 4usize;
        let first = messages[0].clone();
        let tail_start = messages.len().saturating_sub(keep_tail);
        let tail: Vec<Message> = messages[tail_start..].to_vec();

        let mut new_messages = Vec::with_capacity(2 + keep_tail);
        new_messages.push(first);
        new_messages.push(Message::user(
            "[Earlier tool results were truncated to fit the context window. \
             If you need to re-read a file, request it again.]"
                .to_string(),
        ));
        new_messages.extend(tail);

        let new_chars = Self::estimate_message_chars(&new_messages);
        info!(
            old_count = messages.len(),
            new_count = new_messages.len(),
            chars_saved = total_chars.saturating_sub(new_chars),
            "Truncated context"
        );
        *messages = new_messages;
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

    /// Core agentic turn loop shared by both `process_turn` and
    /// `process_turn_with_messages`.
    #[allow(clippy::too_many_lines)]
    async fn run_turn_loop(
        &self,
        mut messages: Vec<Message>,
        agent_id: AgentId,
    ) -> anyhow::Result<TurnResult> {
        let mut entries = Vec::new();
        let mut total_input_tokens = 0u64;
        let mut total_output_tokens = 0u64;
        let mut had_failures = false;
        let mut cancelled = false;
        let mut final_message = None;
        let provider_name = self.provider.name().to_string();
        let model_name = self.config.model.clone();
        let mut tool_cache: ToolCache = HashMap::new();

        for step in 0..self.config.max_steps {
            if self.is_cancelled() {
                info!(step = step, "Turn cancelled before step");
                cancelled = true;
                break;
            }

            debug!(step = step, messages = messages.len(), "Processing step");

            Self::truncate_messages_if_needed(&mut messages, &self.config);

            let step_result = self
                .process_step(&messages, agent_id, &mut tool_cache, &StepConfig::default())
                .await?;

            total_input_tokens += step_result.response.usage.input_tokens;
            total_output_tokens += step_result.response.usage.output_tokens;

            messages.push(step_result.response.message.clone());
            final_message = Some(step_result.response.message.clone());

            if step_result.had_failures {
                had_failures = true;
            }

            match step_result.stop_reason {
                StopReason::EndTurn => {
                    info!(step = step, "Turn completed (end_turn)");
                    entries.push(TurnEntry {
                        turn_step: step,
                        model_response: step_result.response,
                        tool_results: vec![],
                        executed_tools: vec![],
                        stop_reason: StopReason::EndTurn,
                    });
                    break;
                }
                StopReason::ToolUse => {
                    let tool_results: Vec<(String, ToolResultContent, bool)> = step_result
                        .executed_tools
                        .iter()
                        .map(|t| (t.tool_use_id.clone(), t.result.clone(), t.is_error))
                        .collect();

                    entries.push(TurnEntry {
                        turn_step: step,
                        model_response: step_result.response,
                        tool_results: tool_results.clone(),
                        executed_tools: step_result.executed_tools,
                        stop_reason: StopReason::ToolUse,
                    });

                    if !tool_results.is_empty() {
                        messages.push(Message::tool_results(tool_results));
                    }
                }
                StopReason::MaxTokens => {
                    warn!(step = step, "Turn stopped due to max_tokens");
                    entries.push(TurnEntry {
                        turn_step: step,
                        model_response: step_result.response,
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
                        model_response: step_result.response,
                        tool_results: vec![],
                        executed_tools: vec![],
                        stop_reason: StopReason::StopSequence,
                    });
                    break;
                }
            }
        }

        #[allow(clippy::cast_possible_truncation)]
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
            cancelled,
            model: model_name,
            provider: provider_name,
        })
    }

    /// Complete a model request with streaming, emitting events to the callback.
    #[allow(clippy::too_many_lines)]
    async fn complete_with_streaming(
        &self,
        request: ModelRequest,
    ) -> anyhow::Result<ModelResponse> {
        let start = std::time::Instant::now();

        let mut stream = match self.provider.complete_streaming(request).await {
            Ok(s) => s,
            Err(e) => {
                let err_msg = e.to_string();
                let (code, recoverable) = classify_llm_error(&err_msg);
                self.emit_stream_event(StreamCallbackEvent::Error {
                    code: code.to_string(),
                    message: err_msg,
                    recoverable,
                });
                return Err(e);
            }
        };

        // Accumulate the response while emitting text deltas
        let mut accumulator = StreamAccumulator::new();
        let input_tokens = 0u64;
        let mut in_thinking_block = false;

        loop {
            let event_result = if let Some(token) = &self.cancellation_token {
                tokio::select! {
                    biased;
                    () = token.cancelled() => {
                        info!("Stream cancelled by cancellation token");
                        break;
                    }
                    next = stream.next() => next,
                }
            } else {
                stream.next().await
            };

            let Some(event_result) = event_result else {
                break;
            };

            match event_result {
                Ok(event) => {
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
                            if in_thinking_block {
                                self.emit_stream_event(StreamCallbackEvent::ThinkingComplete);
                                in_thinking_block = false;
                            }
                        }
                        StreamEvent::ThinkingDelta { thinking } => {
                            self.emit_stream_event(StreamCallbackEvent::ThinkingDelta(
                                thinking.clone(),
                            ));
                        }
                        StreamEvent::TextDelta { text } => {
                            self.emit_stream_event(StreamCallbackEvent::TextDelta(text.clone()));
                        }
                        StreamEvent::InputJsonDelta { .. } => {
                            // Processed after accumulator.process() below
                        }
                        StreamEvent::Error { message } => {
                            error!(error = %message, "Stream error from provider");
                            let (code, recoverable) = classify_llm_error(message);
                            self.emit_stream_event(StreamCallbackEvent::Error {
                                code: code.to_string(),
                                message: message.clone(),
                                recoverable,
                            });
                            anyhow::bail!("Stream error: {message}");
                        }
                        _ => {}
                    }

                    accumulator.process(&event);

                    if matches!(event, StreamEvent::InputJsonDelta { .. }) {
                        if let Some(tool) = &accumulator.current_tool_use {
                            self.emit_stream_event(StreamCallbackEvent::ToolInputSnapshot {
                                id: tool.id.clone(),
                                name: tool.name.clone(),
                                input: tool.input_json.clone(),
                            });
                        }
                    }

                    if matches!(event, StreamEvent::MessageStop) {
                        break;
                    }
                }
                Err(e) => {
                    error!(error = %e, "Stream error");
                    let err_msg = e.to_string();
                    let (code, recoverable) = classify_llm_error(&err_msg);
                    self.emit_stream_event(StreamCallbackEvent::Error {
                        code: code.to_string(),
                        message: err_msg.clone(),
                        recoverable,
                    });
                    anyhow::bail!("Stream error: {err_msg}");
                }
            }
        }

        // Signal step completion
        self.emit_stream_event(StreamCallbackEvent::StepComplete);

        let latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

        // Convert accumulated state to a ModelResponse
        accumulator.into_response(input_tokens, latency_ms)
    }

    /// Execute tool calls from a model message concurrently, with caching.
    ///
    /// Policy checks are performed synchronously first. Cacheable read-only
    /// tools are checked against `tool_cache`; cache hits are returned without
    /// re-execution. Permitted tools are then executed in parallel via
    /// `futures::future::join_all`. Successful cacheable results are stored
    /// back into the cache for future steps within the same turn.
    #[allow(clippy::too_many_lines)]
    async fn execute_tool_calls(
        &self,
        message: &Message,
        agent_id: AgentId,
        tool_cache: &mut ToolCache,
    ) -> anyhow::Result<Vec<ExecutedToolCall>> {
        let workspace = self.agent_workspace(&agent_id);

        // Ensure workspace exists
        if let Err(e) = tokio::fs::create_dir_all(&workspace).await {
            error!(error = %e, "Failed to create workspace");
        }

        // Phase 1: policy checks + cache lookups
        let mut denied = Vec::new();
        let mut cached = Vec::new();
        let mut to_execute: Vec<(String, String, serde_json::Value)> = Vec::new();

        for block in &message.content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                debug!(tool = %name, id = %id, "Checking tool permission");

                let permission = self.policy.check_tool_permission(name);
                match permission {
                    PermissionLevel::Deny => {
                        warn!(tool = %name, "Tool denied by policy");
                        denied.push(ExecutedToolCall {
                            tool_use_id: id.clone(),
                            tool_name: name.clone(),
                            tool_args: input.clone(),
                            result: ToolResultContent::text(format!(
                                "Tool '{name}' is not allowed"
                            )),
                            is_error: true,
                            metadata: HashMap::default(),
                        });
                        continue;
                    }
                    PermissionLevel::AlwaysAsk => {
                        debug!(tool = %name, "Tool requires approval (AlwaysAsk)");
                    }
                    PermissionLevel::AskOnce => {
                        debug!(tool = %name, "Tool allowed (AskOnce)");
                    }
                    PermissionLevel::AlwaysAllow => {
                        debug!(tool = %name, "Tool allowed (AlwaysAllow)");
                    }
                }

                // Check cache for read-only tools
                if CACHEABLE_TOOLS.contains(&name.as_str()) {
                    let cache_key = make_cache_key(name, input);
                    if let Some(hit) = tool_cache.get(&cache_key) {
                        debug!(tool = %name, "Cache hit — returning cached result");
                        let mut cloned = hit.clone();
                        cloned.tool_use_id.clone_from(id);
                        cached.push(cloned);
                        continue;
                    }
                }

                to_execute.push((id.clone(), name.clone(), input.clone()));
            }
        }

        // Phase 2: execute permitted tools in parallel
        let futures: Vec<_> = to_execute
            .into_iter()
            .map(|(id, name, input)| {
                let workspace = workspace.clone();
                async move {
                    let tool_call = ToolCall::new(name.clone(), input.clone());
                    let action = Action::delegate_tool(&tool_call);
                    let ctx = ExecuteContext::new(agent_id, action.action_id, workspace);

                    let effect = self.executor.execute(&ctx, &action).await;

                    if effect.status == EffectStatus::Committed {
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
                            ExecutedToolCall {
                                tool_use_id: id,
                                tool_name: name,
                                tool_args: input,
                                result: content,
                                is_error: !tool_result.ok,
                                metadata: tool_result.metadata,
                            }
                        } else {
                            ExecutedToolCall {
                                tool_use_id: id,
                                tool_name: name,
                                tool_args: input,
                                result: ToolResultContent::text("Tool executed successfully"),
                                is_error: false,
                                metadata: HashMap::default(),
                            }
                        }
                    } else {
                        let error_msg = if let Ok(tool_result) =
                            serde_json::from_slice::<ToolResult>(&effect.payload)
                        {
                            String::from_utf8_lossy(&tool_result.stderr).to_string()
                        } else {
                            "Tool execution failed".to_string()
                        };
                        ExecutedToolCall {
                            tool_use_id: id,
                            tool_name: name,
                            tool_args: input,
                            result: ToolResultContent::text(error_msg),
                            is_error: true,
                            metadata: HashMap::default(),
                        }
                    }
                }
            })
            .collect();

        let executed = futures_util::future::join_all(futures).await;

        // Phase 3: populate cache with successful cacheable results
        for result in &executed {
            if !result.is_error && CACHEABLE_TOOLS.contains(&result.tool_name.as_str()) {
                let cache_key = make_cache_key(&result.tool_name, &result.tool_args);
                tool_cache.insert(cache_key, result.clone());
            }
        }

        // Combine denied + cached + executed
        let mut results = denied;
        results.extend(cached);
        results.extend(executed);
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

/// Build a deterministic cache key from tool name and arguments.
///
/// Uses canonical JSON serialization so equivalent argument objects
/// produce the same key regardless of property ordering.
fn make_cache_key(tool_name: &str, args: &serde_json::Value) -> String {
    let canonical = serde_json::to_string(args).unwrap_or_default();
    format!("{tool_name}\0{canonical}")
}

/// Classify an LLM error message into a machine-readable code and recoverability.
///
/// Returns `(code, recoverable)`. Common codes:
/// - `"rate_limit"` — 429 / rate-limited, recoverable (retry after backoff)
/// - `"auth_error"` — 401/403, not recoverable without config change
/// - `"overloaded"` — 529 / server overloaded, recoverable
/// - `"timeout"` — request timed out, recoverable
/// - `"llm_error"` — catch-all for other LLM failures
fn classify_llm_error(message: &str) -> (&'static str, bool) {
    let lower = message.to_lowercase();
    if lower.contains("rate limit") || lower.contains("429") || lower.contains("too many requests")
    {
        ("rate_limit", true)
    } else if lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("authentication")
    {
        ("auth_error", false)
    } else if lower.contains("overloaded") || lower.contains("529") || lower.contains("503") {
        ("overloaded", true)
    } else if lower.contains("timeout") || lower.contains("timed out") {
        ("timeout", true)
    } else {
        ("llm_error", true)
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
