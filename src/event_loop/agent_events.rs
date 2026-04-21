use aura_agent::AgentLoopEvent;
use aura_terminal::{events::ToolData, UiCommand};
use tokio::sync::mpsc;
use tracing::debug;

/// Tracks forwarder lifecycle so the caller can finalize streaming/thinking.
pub(super) struct ForwarderState {
    pub streaming_active: bool,
    pub thinking_active: bool,
    pub had_text: bool,
}

/// Reads [`AgentLoopEvent`]s and translates them into [`UiCommand`]s.
pub(super) async fn forward_agent_events(
    mut rx: tokio::sync::mpsc::Receiver<AgentLoopEvent>,
    commands: mpsc::Sender<UiCommand>,
) -> ForwarderState {
    let mut state = ForwarderState {
        streaming_active: false,
        thinking_active: false,
        had_text: false,
    };

    while let Some(event) = rx.recv().await {
        match event {
            AgentLoopEvent::ThinkingDelta(text) => {
                if !state.thinking_active {
                    let _ = commands.send(UiCommand::StartThinking).await;
                    state.thinking_active = true;
                }
                let _ = commands.send(UiCommand::AppendThinking(text)).await;
            }
            AgentLoopEvent::TextDelta(text) => {
                if state.thinking_active {
                    let _ = commands.send(UiCommand::FinishThinking).await;
                    state.thinking_active = false;
                }
                if !state.streaming_active {
                    let _ = commands.send(UiCommand::StartStreaming).await;
                    state.streaming_active = true;
                }
                state.had_text = true;
                let _ = commands.send(UiCommand::AppendText(text)).await;
            }
            AgentLoopEvent::ToolStart { id, name } => {
                if state.thinking_active {
                    let _ = commands.send(UiCommand::FinishThinking).await;
                    state.thinking_active = false;
                }
                let _ = commands
                    .send(UiCommand::ShowTool(ToolData {
                        id,
                        name,
                        args: String::new(),
                    }))
                    .await;
            }
            AgentLoopEvent::ToolInputSnapshot { id, .. } => {
                debug!(tool_id = %id, "Tool input streaming");
            }
            AgentLoopEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => {
                let _ = commands
                    .send(UiCommand::CompleteTool {
                        id: tool_use_id,
                        result: content,
                        success: !is_error,
                    })
                    .await;
            }
            AgentLoopEvent::IterationComplete { .. }
            | AgentLoopEvent::ThinkingComplete
            | AgentLoopEvent::StepComplete
            | AgentLoopEvent::ToolComplete { .. }
            | AgentLoopEvent::Debug(_) => {}
            AgentLoopEvent::StreamReset { reason } => {
                debug!(reason = %reason, "Stream reset received");
                if state.streaming_active {
                    let _ = commands.send(UiCommand::FinishStreaming).await;
                    state.streaming_active = false;
                }
                if state.thinking_active {
                    let _ = commands.send(UiCommand::FinishThinking).await;
                    state.thinking_active = false;
                }
                state.had_text = false;
            }
            AgentLoopEvent::Warning(msg) => {
                let _ = commands.send(UiCommand::ShowWarning(msg)).await;
            }
            AgentLoopEvent::Error { message, .. } => {
                let _ = commands.send(UiCommand::ShowWarning(message)).await;
            }
        }
    }

    state
}
