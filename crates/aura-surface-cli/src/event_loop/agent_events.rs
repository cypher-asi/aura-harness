//! TUI forwarder: translates [`AgentLoopEvent`]s into [`UiCommand`]s.
//!
//! The per-variant routing lives in
//! [`aura_agent::map_agent_loop_event`]; this module only provides the
//! [`UiCommandSink`] implementation that owns the TUI-specific state
//! machine (streaming / thinking toggles, had-text tracking) and the
//! `mpsc::Sender<UiCommand>` that feeds the terminal.

use async_trait::async_trait;
use aura_agent::{map_agent_loop_event, AgentLoopEvent, DebugEvent, TurnEventSink};
use aura_model_reasoner::{model_catalog, Usage};
use aura_terminal::{
    events::{CallUsage, ToolData},
    UiCommand,
};
use tokio::sync::mpsc;
use tracing::debug;

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
    commands: mpsc::Sender<UiCommand>,
    state: ForwarderState,
    /// Tool calls shown in the chat that have no result yet
    open_tools: Vec<String>,
}

impl UiCommandSink {
    async fn finish_thinking_if_active(&mut self) {
        if self.state.thinking_active {
            let _ = self.commands.send(UiCommand::FinishThinking).await;
            self.state.thinking_active = false;
        }
    }

    /// End the reply message being streamed, so text that follows a tool
    /// call or more thinking starts a new message below them.
    async fn finish_streaming_if_active(&mut self) {
        if self.state.streaming_active {
            let _ = self.commands.send(UiCommand::FinishStreaming).await;
            self.state.streaming_active = false;
        }
    }
}

#[async_trait]
impl TurnEventSink for UiCommandSink {
    async fn on_thinking_delta(&mut self, text: String) {
        if !self.state.thinking_active {
            self.finish_streaming_if_active().await;
            let _ = self.commands.send(UiCommand::StartThinking).await;
            self.state.thinking_active = true;
        }
        let _ = self.commands.send(UiCommand::AppendThinking(text)).await;
    }

    async fn on_thinking_complete(&mut self) {
        self.finish_thinking_if_active().await;
    }

    async fn on_text_delta(&mut self, text: String) {
        self.finish_thinking_if_active().await;
        if !self.state.streaming_active {
            let _ = self.commands.send(UiCommand::StartStreaming).await;
            self.state.streaming_active = true;
        }
        self.state.had_text = true;
        let _ = self.commands.send(UiCommand::AppendText(text)).await;
    }

    async fn on_tool_start(&mut self, id: String, name: String) {
        self.finish_thinking_if_active().await;
        self.finish_streaming_if_active().await;
        self.open_tools.push(id.clone());
        let _ = self
            .commands
            .send(UiCommand::ShowTool(ToolData {
                id,
                name,
                args: String::new(),
            }))
            .await;
    }

    async fn on_tool_input_snapshot(&mut self, id: String, _name: String, input: String) {
        let _ = self
            .commands
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
        let _ = self
            .commands
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
        let _ = self
            .commands
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
        let _ = self
            .commands
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
            let _ = self
                .commands
                .send(UiCommand::CompleteTool {
                    id,
                    result: reason.clone(),
                    success: false,
                })
                .await;
        }
    }

    async fn on_warning(&mut self, message: String) {
        let _ = self.commands.send(UiCommand::ShowWarning(message)).await;
    }

    async fn on_error(&mut self, _code: String, message: String, _recoverable: bool) {
        let _ = self.commands.send(UiCommand::ShowWarning(message)).await;
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
            let usage = call_usage(
                &model,
                &Usage {
                    input_tokens,
                    output_tokens,
                    cache_creation_input_tokens,
                    cache_read_input_tokens,
                },
            );
            let _ = self
                .commands
                .send(UiCommand::SetReplyModel {
                    model: model.clone(),
                    usage,
                })
                .await;
            self.state.reply_model = Some(model);
            self.state.reply_usage = usage;
        }
    }

    // `IterationComplete`, `StepComplete`, `ToolComplete` — intentionally
    // use the default no-op impls; the TUI derives UI transitions from the
    // events above.
}

/// Reads [`AgentLoopEvent`]s and translates them into [`UiCommand`]s.
pub(crate) async fn forward_agent_events(
    mut rx: tokio::sync::mpsc::Receiver<AgentLoopEvent>,
    commands: mpsc::Sender<UiCommand>,
    recorder: Option<std::sync::Arc<aura_agent_trace::RunRecorder>>,
) -> ForwarderState {
    let mut sink = UiCommandSink {
        commands,
        state: ForwarderState::default(),
        open_tools: Vec::new(),
    };

    while let Some(event) = rx.recv().await {
        if let Some(rec) = &recorder {
            rec.on_event(&event);
        }
        map_agent_loop_event(event, &mut sink).await;
    }

    sink.state
}
