//! TUI forwarder: translates [`AgentLoopEvent`]s into [`UiCommand`]s.
//!
//! The per-variant routing lives in
//! [`aura_agent::map_agent_loop_event`]; this module only provides the
//! [`UiCommandSink`] implementation that owns the TUI-specific state
//! machine (streaming / thinking toggles, had-text tracking) and the
//! [`AgentUi`] that feeds the chat of the agent whose turn it is.

use super::hud::{call_stats, todo_views};
use async_trait::async_trait;
use aura_agent::{
    map_agent_loop_event, AgentLoopEvent, DebugEvent, TurnEventSink, PROGRESS_CONTEXT_SUMMARIZED,
    PROGRESS_TODOS_UPDATED,
};
use aura_core_types::AgentId;
use aura_model_reasoner::{model_catalog, Usage};
use aura_terminal::{
    events::{CallUsage, ToolData},
    UiCommand,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;

/// The chat of one agent. Agents run in parallel, so everything a turn
/// shows is addressed to its agent; the UI keeps it with that agent's chat
/// whether or not it is on screen.
#[derive(Clone)]
pub(crate) struct AgentUi {
    commands: mpsc::Sender<UiCommand>,
    agent_id: String,
}

impl AgentUi {
    pub(crate) fn new(commands: mpsc::Sender<UiCommand>, agent_id: AgentId) -> Self {
        Self {
            commands,
            agent_id: agent_id.to_hex(),
        }
    }

    /// Send `command` to this agent's chat.
    pub(crate) async fn send(&self, command: UiCommand) {
        let _ = self
            .commands
            .send(UiCommand::ForAgent {
                agent_id: self.agent_id.clone(),
                command: Box::new(command),
            })
            .await;
    }

    /// The UI's command channel, for panels that aren't per chat.
    pub(crate) const fn commands(&self) -> &mpsc::Sender<UiCommand> {
        &self.commands
    }
}

/// Tracks forwarder lifecycle so the caller can finalize streaming/thinking.
#[derive(Default)]
pub(crate) struct ForwarderState {
    pub streaming_active: bool,
    pub thinking_active: bool,
    pub had_text: bool,
    /// Model that answered the latest model call of the turn
    pub reply_model: Option<String>,
    /// Tokens and cost of the latest model call of the turn
    pub reply_usage: Option<CallUsage>,
    /// Estimated cost of every model call of the turn so far
    pub turn_cost_usd: f64,
    /// Context window of the turn's model
    pub context_limit: Option<u64>,
}

/// Stops a turn once its model calls cost more than `limit_usd`.
pub(crate) struct CostCap {
    pub limit_usd: f64,
    pub stop: CancellationToken,
}

/// Tokens and estimated cost of one model call, for the reply header.
/// `None` when the provider reported no usage at all.
pub(crate) fn call_usage(model: &str, usage: &Usage) -> Option<CallUsage> {
    let tokens = model_catalog::call_tokens(model, usage);
    (tokens > 0).then(|| CallUsage {
        tokens,
        cost_usd: model_catalog::estimate_cost_usd(model, usage),
    })
}

/// Sink implementation that drains [`AgentLoopEvent`]s from the agent
/// loop and fans them out to the TUI as [`UiCommand`]s while maintaining
/// the streaming / thinking state machine the Ratatui layer depends on.
struct UiCommandSink {
    ui: AgentUi,
    state: ForwarderState,
    /// Tool calls shown in the chat that have no result yet
    open_tools: Vec<String>,
    cost_cap: Option<CostCap>,
}

impl UiCommandSink {
    async fn finish_thinking_if_active(&mut self) {
        if self.state.thinking_active {
            self.ui.send(UiCommand::FinishThinking).await;
            self.state.thinking_active = false;
        }
    }

    /// End the reply message being streamed, so text that follows a tool
    /// call or more thinking starts a new message below them.
    async fn finish_streaming_if_active(&mut self) {
        if self.state.streaming_active {
            self.ui.send(UiCommand::FinishStreaming).await;
            self.state.streaming_active = false;
        }
    }

    async fn enforce_cost_cap(&mut self) {
        let Some(cap) = &self.cost_cap else {
            return;
        };
        if self.state.turn_cost_usd <= cap.limit_usd || cap.stop.is_cancelled() {
            return;
        }
        cap.stop.cancel();
        let message = format!(
            "Stopping this turn: it has cost about ${:.2}, over the ${:.2} limit \
             (AURA_TUI_MAX_TURN_COST_USD)",
            self.state.turn_cost_usd, cap.limit_usd
        );
        self.ui.send(UiCommand::ShowWarning(message)).await;
    }
}

#[async_trait]
impl TurnEventSink for UiCommandSink {
    async fn on_thinking_delta(&mut self, text: String) {
        if !self.state.thinking_active {
            self.finish_streaming_if_active().await;
            self.ui.send(UiCommand::StartThinking).await;
            self.state.thinking_active = true;
        }
        self.ui.send(UiCommand::AppendThinking(text)).await;
    }

    async fn on_thinking_complete(&mut self) {
        self.finish_thinking_if_active().await;
    }

    async fn on_text_delta(&mut self, text: String) {
        self.finish_thinking_if_active().await;
        if !self.state.streaming_active {
            self.ui.send(UiCommand::StartStreaming).await;
            self.state.streaming_active = true;
        }
        self.state.had_text = true;
        self.ui.send(UiCommand::AppendText(text)).await;
    }

    async fn on_tool_start(&mut self, id: String, name: String) {
        self.finish_thinking_if_active().await;
        self.finish_streaming_if_active().await;
        self.open_tools.push(id.clone());
        self.ui
            .send(UiCommand::ShowTool(ToolData {
                id,
                name,
                args: String::new(),
            }))
            .await;
    }

    async fn on_tool_input_snapshot(&mut self, id: String, _name: String, input: String) {
        self.ui
            .send(UiCommand::UpdateToolArgs { id, args: input })
            .await;
    }

    async fn on_tool_result(
        &mut self,
        tool_use_id: String,
        _tool_name: String,
        content: String,
        is_error: bool,
        _image: Option<aura_core_types::ToolResultImage>,
    ) {
        self.open_tools.retain(|id| *id != tool_use_id);
        self.ui
            .send(UiCommand::CompleteTool {
                id: tool_use_id,
                result: content,
                success: !is_error,
            })
            .await;
    }

    async fn on_tool_call_retrying(
        &mut self,
        tool_use_id: String,
        _tool_name: String,
        attempt: u32,
        max_attempts: u32,
        _delay_ms: u64,
        reason: String,
    ) {
        debug!(tool_id = %tool_use_id, %reason, "Tool call retrying");
        self.ui
            .send(UiCommand::ToolRetrying {
                id: tool_use_id,
                attempt,
                max_attempts,
            })
            .await;
    }

    async fn on_tool_call_failed(
        &mut self,
        tool_use_id: String,
        _tool_name: String,
        reason: String,
    ) {
        self.open_tools.retain(|id| *id != tool_use_id);
        self.ui
            .send(UiCommand::CompleteTool {
                id: tool_use_id,
                result: reason,
                success: false,
            })
            .await;
    }

    async fn on_stream_reset(
        &mut self,
        reason: String,
        _text_bytes: usize,
        _thinking_bytes: usize,
    ) {
        debug!(reason = %reason, "Stream reset received");
        self.finish_streaming_if_active().await;
        self.finish_thinking_if_active().await;
        self.state.had_text = false;
        // The model starts its response over, so calls it was still
        // writing will never get a result.
        for id in std::mem::take(&mut self.open_tools) {
            self.ui
                .send(UiCommand::CompleteTool {
                    id,
                    result: reason.clone(),
                    success: false,
                })
                .await;
        }
    }

    async fn on_warning(&mut self, message: String) {
        self.ui.send(UiCommand::ShowWarning(message)).await;
    }

    async fn on_error(&mut self, _code: String, message: String, _recoverable: bool) {
        self.ui.send(UiCommand::ShowWarning(message)).await;
    }

    async fn on_debug(&mut self, event: DebugEvent) {
        if let DebugEvent::LlmCall {
            model,
            input_tokens,
            output_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
            ..
        } = event
        {
            if model.is_empty() {
                return;
            }
            let usage = Usage {
                input_tokens,
                output_tokens,
                cache_creation_input_tokens,
                cache_read_input_tokens,
            };
            self.ui
                .send(UiCommand::CallStats(call_stats(
                    &model,
                    &usage,
                    self.state.context_limit,
                )))
                .await;
            let usage = call_usage(&model, &usage);
            self.ui
                .send(UiCommand::SetReplyModel {
                    model: model.clone(),
                    usage,
                })
                .await;
            self.state.reply_model = Some(model);
            self.state.reply_usage = usage;
            self.state.turn_cost_usd += usage.and_then(|u| u.cost_usd).unwrap_or(0.0);
            self.enforce_cost_cap().await;
        }
    }

    async fn on_progress(
        &mut self,
        stage: String,
        _tool_name: Option<String>,
        _elapsed_ms: Option<u64>,
        message: Option<String>,
    ) {
        let Some(message) = message else {
            return;
        };
        match stage.as_str() {
            PROGRESS_TODOS_UPDATED => {
                match serde_json::from_str::<Vec<aura_tools::todo::TodoItem>>(&message) {
                    Ok(todos) => self.ui.send(UiCommand::SetTodos(todo_views(&todos))).await,
                    Err(e) => debug!(error = %e, "Unreadable todo list update"),
                }
            }
            PROGRESS_CONTEXT_SUMMARIZED => self.ui.send(UiCommand::Compacted(message)).await,
            _ => {}
        }
    }

    // `IterationComplete`, `StepComplete`, `ToolComplete` — intentionally
    // use the default no-op impls; the TUI derives UI transitions from the
    // events above.
}

/// Reads [`AgentLoopEvent`]s and translates them into [`UiCommand`]s for
/// the chat of `ui`'s agent.
pub(crate) async fn forward_agent_events(
    mut rx: tokio::sync::mpsc::Receiver<AgentLoopEvent>,
    ui: AgentUi,
    cost_cap: Option<CostCap>,
    context_limit: Option<u64>,
) -> ForwarderState {
    let mut sink = UiCommandSink {
        ui,
        state: ForwarderState {
            context_limit,
            ..ForwarderState::default()
        },
        open_tools: Vec::new(),
        cost_cap,
    };

    while let Some(event) = rx.recv().await {
        map_agent_loop_event(event, &mut sink).await;
    }

    sink.state
}
