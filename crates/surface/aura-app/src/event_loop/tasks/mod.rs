//! Tasks: the work agents did or are doing, kept in the workspace under
//! `.aura/tasks/<id>.json` so the TASKS tab shows them after a restart.
//!
//! A user's request becomes a task once its turn does something (its
//! first tool call; a turn that only answers is not one), and each `task`
//! call becomes a task under the caller's, done by the agent it spawned.
//! A task's items follow its agent's todo list, so it says what is done
//! and what is left; it links to the plan it works on, and a spawned task
//! to that plan's todo it was handed.
//!
//! Plans and requests:
//! - The request that starts building a plan (`/build`, or the user
//!   saying "implement") is marked [`TaskView::builds`]: the TASKS tab
//!   shows it as the plan's header rather than as a task of its own.
//! - Every request while the agent builds its plan links to that plan,
//!   so follow-ups ("commit", "keep going") file under it too.
//!
//! A harness notice (a background command or agent ending) is not a
//! request: it starts no task. Its turn reopens the agent's latest task
//! in that workspace, marks it running and settles it when the turn ends,
//! as a request's turn would ([`TaskBoard::resume_request`]). An agent
//! with no task gets none.

use super::record_ui::RecordPanelAgent;
use super::subagents::{lock, AgentHub};
use aura_core_types::time::now_ms;
use aura_core_types::AgentId;
use aura_ui::events::{PlanView, TaskState, TaskView, TodoState, TodoView};
use aura_ui::UiCommand;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::warn;

mod board;
mod state;
pub use board::*;
use state::*;
pub(crate) use state::{send_channel, TasksAside};

#[cfg(test)]
mod tests;

/// Workspace-relative folder the tasks are kept in.
pub(crate) const TASKS_DIR: &str = ".aura/tasks";

/// Longest title kept, in characters.
const TITLE_CHARS: usize = 120;
/// Longest summary kept, in characters.
const SUMMARY_CHARS: usize = 400;

/// One task as kept on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TaskRecord {
    #[serde(flatten)]
    pub view: TaskView,
    /// When it started, in ms since the epoch
    pub created_ms: u64,
    /// The `task` call it answers, for a spawned task
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
}

/// A new task id: when it started, and a count that keeps ids made in
/// the same millisecond apart.
fn new_id(now: u64) -> String {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let count = COUNT.fetch_add(1, Ordering::Relaxed) & 0xffff;
    format!("{now:012x}{count:04x}")
}

/// The first line of `text`, cut to `max` characters.
fn first_line(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    cut(line, max)
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// What a reply says it did: its first paragraph, cut short.
pub(crate) fn summary_of(reply: &str) -> Option<String> {
    let paragraph = reply
        .trim()
        .split("\n\n")
        .next()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!paragraph.is_empty()).then(|| cut(&paragraph, SUMMARY_CHARS))
}

/// Write `record` into the folder `dir`, whole or not at all.
fn save_in(dir: &Path, record: &TaskRecord) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", record.view.id));
    let json = serde_json::to_vec_pretty(record).map_err(std::io::Error::other)?;
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, json)?;
    std::fs::rename(&partial, &path)
}

/// Every readable task in `workspace`, newest first.
pub(crate) fn load_all(workspace: &Path) -> Vec<TaskRecord> {
    load_from(&workspace.join(TASKS_DIR))
}

/// Every readable task in the folder `dir`, newest first.
fn load_from(dir: &Path) -> Vec<TaskRecord> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut records: Vec<TaskRecord> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| std::fs::read(&path).ok())
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect();
    records.sort_by(|a, b| (b.created_ms, &b.view.id).cmp(&(a.created_ms, &a.view.id)));
    records
}

/// For each plan, by workspace-relative path, the newest task doing each
/// of its todos.
#[derive(Debug, Default)]
pub(crate) struct PlanTasks(HashMap<String, HashMap<String, String>>);

impl PlanTasks {
    /// The tasks kept in `workspace`.
    pub(crate) fn load(workspace: &Path) -> Self {
        let mut plans: HashMap<String, HashMap<String, String>> = HashMap::new();
        for record in load_all(workspace) {
            let view = record.view;
            if let (Some(plan), Some(todo)) = (view.plan, view.plan_todo) {
                plans
                    .entry(plan)
                    .or_default()
                    .entry(todo)
                    .or_insert(view.id);
            }
        }
        Self(plans)
    }

    /// The newest task doing todo `todo` of the plan at `plan`.
    pub(crate) fn task(&self, plan: &str, todo: &str) -> Option<&str> {
        self.0.get(plan)?.get(todo).map(String::as_str)
    }
}

/// How a request ended, given whether it was cancelled or failed and
/// what is left on its list.
pub(crate) fn request_state(cancelled: bool, failed: bool, items: &[TodoView]) -> TaskState {
    if cancelled {
        TaskState::Cancelled
    } else if failed {
        TaskState::Failed
    } else if items.iter().any(|item| !item.state.is_closed()) {
        TaskState::Open
    } else {
        TaskState::Completed
    }
}
