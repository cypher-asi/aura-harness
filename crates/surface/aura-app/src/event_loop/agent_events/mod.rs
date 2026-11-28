//! TUI forwarder: translates [`AgentLoopEvent`]s into [`UiCommand`]s.
//!
//! The per-variant routing lives in
//! [`aura_agent::map_agent_loop_event`]; this module only provides the
//! [`UiCommandSink`] implementation that owns the TUI-specific state
//! machine (streaming / thinking toggles, had-text tracking) and the
//! [`AgentUi`] that feeds the chat of the agent whose turn it is.

use super::hud::{call_session_usage, call_stats, todo_views};
use super::plan_mode::{plan_view, plan_views, Build};
use super::record_ui::RecordPanelAgent;
use crate::session_checkpoint::SessionUsage;
use async_trait::async_trait;
use aura_agent::{
    map_agent_loop_event, AgentLoopEvent, DebugEvent, TurnEventSink, ERROR_INSUFFICIENT_CREDITS,
    ERROR_UNAUTHORIZED, PROGRESS_CONTEXT_SUMMARIZED, PROGRESS_TODOS_UPDATED,
};
use aura_core_types::AgentId;
use aura_exec_tools::plan_tools::{plan_path_from_output, sync_todo_list, CREATE_PLAN_TOOL_NAME};
use aura_exec_tools::todo::TodoItem;
use aura_gate::compare::ModelCall;
use aura_model::{model_catalog, Usage};
use aura_ui::{
    events::{CallUsage, MessageData, MessageRole, ToolData},
    UiCommand,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

mod sink;
use sink::*;

/// The chat of one agent. Agents run in parallel, so everything a turn
/// shows is addressed to its agent; the UI keeps it with that agent's chat
/// whether or not it is on screen.
#[derive(Clone)]
pub(crate) struct AgentUi {
    commands: mpsc::Sender<UiCommand>,
    agent_id: AgentId,
    /// Which agent is on screen, for panels the UI keeps for that agent
    /// only (the plan strip)
    shown: Option<RecordPanelAgent>,
}

impl AgentUi {
    pub(crate) const fn new(commands: mpsc::Sender<UiCommand>, agent_id: AgentId) -> Self {
        Self {
            commands,
            agent_id,
            shown: None,
        }
    }

    /// Let [`Self::send_if_shown`] know which agent is on screen.
    #[must_use]
    pub(crate) fn with_shown(mut self, shown: RecordPanelAgent) -> Self {
        self.shown = Some(shown);
        self
    }

    /// Send `command` for a panel the UI only keeps for the agent on
    /// screen, when that is this agent.
    pub(crate) async fn send_if_shown(&self, command: UiCommand) {
        if self
            .shown
            .as_ref()
            .is_some_and(|shown| shown.shows(self.agent_id))
        {
            let _ = self.commands.send(command).await;
        }
    }

    /// Whether this agent's chat may be on screen: it is the agent shown,
    /// or which one is shown isn't known.
    fn maybe_on_screen(&self) -> bool {
        self.shown
            .as_ref()
            .is_none_or(|shown| shown.shows(self.agent_id))
    }

    /// Send `command` for the whole app (the model selector and status
    /// line), not this agent's chat.
    pub(crate) async fn send_app(&self, command: UiCommand) {
        let _ = self.commands.send(command).await;
    }

    /// Send `command` to this agent's chat.
    pub(crate) async fn send(&self, command: UiCommand) {
        let _ = self.commands.send(self.for_agent(command)).await;
    }

    /// [`Self::send`] without waiting: `command` is dropped when the UI
    /// is behind.
    pub(crate) fn try_send(&self, command: UiCommand) {
        let _ = self.commands.try_send(self.for_agent(command));
    }

    /// `command` addressed to this agent's chat, kept in its notes when it
    /// is one, so a chat rebuilt from the record still shows it.
    fn for_agent(&self, command: UiCommand) -> UiCommand {
        crate::chat_notes::record(self.agent_id, &command);
        super::factory::observe::record(self.agent_id, &command);
        UiCommand::ForAgent {
            agent_id: self.agent_id.to_hex(),
            command: Box::new(command),
        }
    }
}

/// Tracks forwarder lifecycle so the caller can finalize streaming/thinking.
#[derive(Default)]
pub(crate) struct ForwarderState {
    pub streaming_active: bool,
    pub thinking_active: bool,
    pub had_text: bool,
    /// Estimated cost of every model call of the turn so far
    pub turn_cost_usd: f64,
    /// Context window of the turn's model
    pub context_limit: Option<u64>,
    /// Model calls and tokens of the turn
    pub usage: SessionUsage,
    /// Plan the turn last wrote with `create_plan`
    pub plan_path: Option<String>,
    /// Whether the model's route refused the user's sign-in or plan
    pub credentials_rejected: bool,
}

/// Stops a turn once its model calls cost more than `limit_usd`.
pub(crate) struct CostCap {
    pub limit_usd: f64,
    pub stop: CancellationToken,
}

/// The plans a turn works with, and the task it does.
pub(crate) struct TurnPlan {
    /// The agent whose turn it is, and where its task is kept
    pub agent: AgentId,
    pub tasks: std::sync::Arc<super::tasks::TaskBoard>,
    /// Folder the agent's tools resolve paths against
    pub workspace: PathBuf,
    /// Workspace-relative plan the agent is building, whose todos follow
    /// its todo list
    pub building: Option<String>,
    /// How a plan the turn writes gets built, which its plan-ready note
    /// says
    pub build: Build,
}

/// Tokens and estimated cost of one model call, for the reply footer:
/// its usage read in the shape of the endpoint it went to, priced as
/// [`ModelCall`] prices it. `None` when the provider reported no usage
/// at all.
pub(crate) fn call_usage(call: &ModelCall) -> Option<CallUsage> {
    let tokens = model_catalog::call_tokens(call.endpoint(), &call.usage);
    (tokens > 0).then(|| CallUsage {
        tokens,
        cost_usd: call.estimate_cost_usd(),
        // A local model's call is free, not unpriced.
        unpriced_calls: u32::from(call.list_price().is_none()),
    })
}

/// How long streamed text may wait to be merged with the text after it.
/// Every agent shares one command channel to the UI, so one command per
/// token would crowd it as agents are added.
/// Status line after a model call: `Iteration 3 · 41.2k tokens`.
fn iteration_status(iteration: usize, tokens: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let tokens = if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    };
    format!("Iteration {} · {tokens} tokens", iteration + 1)
}

/// Reads [`AgentLoopEvent`]s and translates them into [`UiCommand`]s for
/// the chat of `ui`'s agent.
pub(crate) async fn forward_agent_events(
    mut rx: tokio::sync::mpsc::Receiver<AgentLoopEvent>,
    ui: AgentUi,
    cost_cap: Option<CostCap>,
    context_limit: Option<u64>,
    plan: TurnPlan,
    recorder: Option<std::sync::Arc<aura_agent_trace::RunRecorder>>,
) -> ForwarderState {
    let mut sink = UiCommandSink {
        ui,
        state: ForwarderState {
            context_limit,
            ..ForwarderState::default()
        },
        pending: None,
        open_tools: Vec::new(),
        cost_cap,
        plan,
        turn_tokens: 0,
    };

    loop {
        let event = match sink.pending.as_ref().map(|p| p.flush_at) {
            Some(flush_at) => match tokio::time::timeout_at(flush_at.into(), rx.recv()).await {
                Ok(event) => event,
                Err(_) => {
                    sink.flush_delta().await;
                    continue;
                }
            },
            None => rx.recv().await,
        };
        let Some(event) = event else {
            break;
        };
        if let Some(rec) = &recorder {
            rec.on_event(&event);
        }
        if !matches!(
            event,
            AgentLoopEvent::TextDelta(_) | AgentLoopEvent::ThinkingDelta(_)
        ) {
            sink.flush_delta().await;
        }
        map_agent_loop_event(event, &mut sink).await;
    }
    sink.flush_delta().await;

    sink.state
}

#[cfg(test)]
mod tests;
