//! Per-agent sessions, agent switching, and permanent agent management for
//! the terminal UI.
//!
//! Every registered agent gets its own kernel (and therefore its own
//! record) plus its own model conversation. Kernels are opened lazily the
//! first time an agent is selected and kept for the rest of the run, so
//! switching back resumes the in-memory conversation. Switching only
//! changes which agent the UI shows; agents keep working meanwhile.

use super::agent_events::AgentUi;
use super::plan_mode;
use super::side_panels::ms_since;
use super::subagents::{lock, AgentHub, AgentLists};
use super::tasks::TaskBoard;
use super::LoopState;
use crate::agent_registry::{Opened, RegisteredAgent};
use crate::file_checkpoints::FileCheckpoints;
use crate::record_loader::{
    conversation_from, latest_plan_path_in, model_history_from, RecordTail,
};
use crate::session_checkpoint::{is_resumable, Checkpoints, SessionCheckpoint, SessionUsage};
use aura_agent::{
    CheckNotifier, CheckOutcome, CheckReport, CheckSource, KernelModelGateway, KernelToolGateway,
    PostEditCheck, PostEditCheckConfig,
};
use aura_agent_kernel::{Kernel, RecordObserver};
use aura_core_types::time::now_ms;
use aura_core_types::{AgentId, Identity};
use aura_exec_tools::agents::task::TASK_TOOL_NAME;
use aura_exec_tools::todo::{
    apply_todo_write, assign_task, report_task, task_reports, task_todo, TodoItem,
    ASSIGN_TOOL_NAME, TODO_WRITE_TOOL_NAME,
};
use aura_exec_tools::RunScratch;
use aura_model::{ContentBlock, Message, Role};
use aura_store_record::ReadStore;
use aura_ui::events::MessageData;
use aura_ui::UiCommand;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

mod activate;
mod handlers;
mod kernel;
mod worktrees;
pub(crate) use activate::*;
pub(crate) use handlers::*;
pub use kernel::*;

#[cfg(test)]
mod check_notice_tests;

/// Live state for one agent.
pub struct AgentSession {
    pub identity: Identity,
    pub kernel: Arc<Kernel>,
    pub model_gateway: KernelModelGateway,
    pub tool_gateway: KernelToolGateway,
    /// Model conversation for this agent's current session.
    pub messages: Vec<Message>,
    /// Permanent agent this is an instance of; its turns follow its rules.
    pub definition: Option<String>,
    /// The agent's `todo_write` list, carried from turn to turn.
    pub todos: Vec<TodoItem>,
    /// When the current session started; names its checkpoint
    pub started_ms: u64,
    /// Model calls and tokens of the current session
    pub usage: SessionUsage,
    /// Where the session is checkpointed after each turn, if anywhere
    pub checkpoints: Option<Checkpoints>,
    /// Where files are checkpointed before each turn changes them, if anywhere
    pub file_checkpoints: Option<FileCheckpoints>,
    /// Runs the project's check after steps that edit files
    pub post_edit_check: Option<PostEditCheck>,
    /// Workspace-relative path of the plan this agent last wrote, which
    /// `/build` implements.
    pub active_plan: Option<String>,
    /// Whether the agent is building `active_plan` (after `/build`), so its
    /// todo list is mirrored into the plan file.
    pub building_plan: bool,
    /// Whether its current turn runs as Gate's lead with delegation
    /// enforced (see [`aura_gate::lead`]); set where the turn's tools are put
    /// together.
    pub lead: bool,
    /// Where its work is kept as tasks
    pub tasks: Arc<TaskBoard>,
}

impl AgentSession {
    #[must_use]
    pub const fn agent_id(&self) -> AgentId {
        self.identity.agent_id
    }

    /// Start a new, empty session.
    pub fn reset(&mut self) {
        self.messages.clear();
        self.todos.clear();
        self.usage = SessionUsage::default();
        self.started_ms = now_ms();
        self.building_plan = false;
    }

    /// Pick the plan build back up after the session was reopened or
    /// restored: the in-memory flag is gone, but a todo list that is still
    /// the active plan's means `/build` is under way.
    pub fn resume_plan_build(&mut self) {
        self.building_plan = self.active_plan.as_deref().is_some_and(|path| {
            plan_mode::is_plan_todo_list(&self.kernel.agent_workspace(), path, &self.todos)
        });
    }

    /// Continue the session saved in `checkpoint`.
    pub fn restore(&mut self, checkpoint: SessionCheckpoint) {
        self.messages = checkpoint.messages;
        if !is_resumable(&self.messages) {
            self.messages.pop();
        }
        // Kept as saved: a terminal turn drops a finished list before it
        // checkpoints, and a headless `--resume` reports the list it had.
        self.todos = checkpoint.todos;
        self.usage = checkpoint.usage;
        self.started_ms = checkpoint.started_ms;
        self.resume_plan_build();
    }

    /// Checkpoint the session as of the head of its record. A failure is
    /// logged; the record still has the conversation.
    pub async fn save_checkpoint(&self) {
        let Some(checkpoints) = &self.checkpoints else {
            return;
        };
        let agent_id = self.agent_id();
        let record_seq = match self.kernel.store().get_head_seq(agent_id) {
            Ok(seq) => seq,
            Err(e) => {
                warn!(%agent_id, error = %e, "Skipping checkpoint: record head unknown");
                return;
            }
        };
        let checkpoint = SessionCheckpoint::new(
            self.started_ms,
            record_seq,
            self.messages.clone(),
            self.todos.clone(),
            self.usage.clone(),
        );
        if let Err(e) = checkpoints.save(agent_id, &checkpoint).await {
            warn!(%agent_id, error = %e, "Failed to save session checkpoint");
        }
    }
}

/// The todo list left by the `todo_write` calls in `messages`, with the
/// todos `task` and `assign` calls gave to agents assigned and, once the
/// agents reported back (in the call's result, or a later notice or inbox
/// reply), closed: the list the agent loop kept as the turns ran.
pub(crate) fn todos_from_history(messages: &[Message]) -> Vec<TodoItem> {
    let mut todos = Vec::new();
    let mut tasks: HashMap<&str, (&str, &serde_json::Value)> = HashMap::new();
    for message in messages {
        for block in &message.content {
            let next = match block {
                ContentBlock::ToolUse { name, input, .. } if name == TODO_WRITE_TOOL_NAME => {
                    apply_todo_write(&todos, input).ok()
                }
                ContentBlock::ToolUse { id, name, input }
                    if name == TASK_TOOL_NAME || name == ASSIGN_TOOL_NAME =>
                {
                    tasks.insert(id, (name, input));
                    assign_task(&todos, name, input)
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    is_error,
                    ..
                } => tasks
                    .get(tool_use_id.as_str())
                    .and_then(|(name, input)| task_todo(name, input))
                    .filter(|(_, reports)| *reports)
                    .and_then(|(id, _)| report_task(&todos, id, !is_error)),
                ContentBlock::Text { text } if message.role == Role::User => {
                    let mut next = None;
                    for (id, ok) in task_reports(text) {
                        let current = next.as_ref().unwrap_or(&todos);
                        if let Some(reported) = report_task(current, &id, ok) {
                            next = Some(reported);
                        }
                    }
                    next
                }
                _ => None,
            };
            if let Some(next) = next {
                todos = next;
            }
        }
    }
    todos
}
