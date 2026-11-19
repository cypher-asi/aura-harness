//! Tasks: the work agents did or are doing, kept in the workspace under
//! `.aura/tasks/<id>.json` so the TASKS tab shows them after a restart.
//!
//! A user's request becomes a task once its turn does something (its
//! first tool call; a turn that only answers is not one), and each `task`
//! call becomes a task under the caller's, done by the agent it spawned.
//! A task's items follow its agent's todo list, so it says what is done
//! and what is left; it links to the plan it works on, and a spawned task
//! to that plan's todo it was handed.

use super::record_ui::RecordPanelAgent;
use super::subagents::lock;
use aura_core_types::AgentId;
use aura_ui_terminal::events::{TaskState, TaskView, TodoState, TodoView};
use aura_ui_terminal::UiCommand;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use tokio::sync::mpsc;
use tracing::warn;

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

fn now_ms() -> u64 {
    super::processes::now_ms()
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

fn task_path(workspace: &Path, id: &str) -> PathBuf {
    workspace.join(TASKS_DIR).join(format!("{id}.json"))
}

/// Write `record` into `workspace`, whole or not at all.
pub(crate) fn save(workspace: &Path, record: &TaskRecord) -> std::io::Result<()> {
    let path = task_path(workspace, &record.view.id);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(record).map_err(std::io::Error::other)?;
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, json)?;
    std::fs::rename(&partial, &path)
}

/// Every readable task in `workspace`, newest first.
pub(crate) fn load_all(workspace: &Path) -> Vec<TaskRecord> {
    let Ok(entries) = std::fs::read_dir(workspace.join(TASKS_DIR)) else {
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

/// For each todo of the plan at workspace-relative `plan`, the newest
/// task doing it.
pub(crate) fn plan_tasks(workspace: &Path, plan: &str) -> HashMap<String, String> {
    let mut tasks = HashMap::new();
    for record in load_all(workspace) {
        let view = record.view;
        if let (Some(path), Some(todo)) = (view.plan, view.plan_todo) {
            if path == plan {
                tasks.entry(todo).or_insert(view.id);
            }
        }
    }
    tasks
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

/// An agent's task while it works on it.
struct Current {
    /// Where the task is kept
    workspace: PathBuf,
    record: TaskRecord,
    /// Whether it is on disk and shown yet: a request is not until its
    /// turn does something
    saved: bool,
}

#[derive(Default)]
struct Board {
    current: HashMap<AgentId, Current>,
    /// Each agent's workspace, as last shown
    workspaces: HashMap<AgentId, PathBuf>,
    /// Workspaces whose tasks left running by an earlier run were marked
    /// open
    recovered: HashSet<PathBuf>,
}

/// Every agent's current task, kept on disk as it moves on and shown in
/// the TASKS tab of the agent on screen when it is in that agent's
/// workspace.
#[derive(Default)]
pub struct TaskBoard {
    ui: Option<(mpsc::Sender<UiCommand>, RecordPanelAgent)>,
    board: Mutex<Board>,
}

/// What a spawned task is.
pub(crate) struct Spawned<'a> {
    pub parent: AgentId,
    pub child: AgentId,
    /// The child's workspace, for when its parent has no task
    pub workspace: PathBuf,
    pub name: &'a str,
    pub assignee: &'a str,
    pub prompt: &'a str,
    pub tool_use_id: Option<String>,
    pub todo_id: Option<String>,
}

impl TaskBoard {
    /// A board that shows the tasks through `commands`, in the TASKS tab
    /// of the agent `shown` says is on screen.
    #[must_use]
    pub fn new(commands: mpsc::Sender<UiCommand>, shown: RecordPanelAgent) -> Self {
        Self {
            ui: Some((commands, shown)),
            board: Mutex::default(),
        }
    }

    /// `agent`, named `name`, was asked `prompt` by the user in
    /// `workspace`, working on the plan at `plan`. Nothing is kept until
    /// the turn does something ([`Self::touch`]). An agent doing a task
    /// it was handed keeps that one.
    pub(crate) fn begin_request(
        &self,
        agent: AgentId,
        workspace: PathBuf,
        name: &str,
        prompt: &str,
        plan: Option<String>,
    ) {
        let mut board = lock(&self.board);
        if board
            .current
            .get(&agent)
            .is_some_and(|current| current.record.view.parent.is_some())
        {
            return;
        }
        let now = now_ms();
        let record = TaskRecord {
            view: TaskView {
                id: new_id(now),
                title: first_line(prompt, TITLE_CHARS),
                state: TaskState::Running,
                agent_id: agent.to_hex(),
                agent_name: name.to_string(),
                plan,
                updated_ms: now,
                ..TaskView::default()
            },
            created_ms: now,
            tool_use_id: None,
        };
        board.current.insert(
            agent,
            Current {
                workspace,
                record,
                saved: false,
            },
        );
    }

    /// `agent`'s turn did something: keep and show its task.
    pub(crate) async fn touch(&self, agent: AgentId) {
        self.change(agent, |_| {}).await;
    }

    /// Change `agent`'s task with `change`, then keep and show it.
    pub(crate) async fn change(&self, agent: AgentId, change: impl FnOnce(&mut TaskRecord)) {
        let shown = {
            let mut board = lock(&self.board);
            let Some(current) = board.current.get_mut(&agent) else {
                return;
            };
            let before = current.record.clone();
            change(&mut current.record);
            if current.saved && current.record == before {
                return;
            }
            current.record.view.updated_ms = now_ms();
            current.saved = true;
            keep(&current.workspace, &current.record);
            (current.workspace.clone(), current.record.view.clone())
        };
        self.show(shown).await;
    }

    /// `agent`'s todo list is now `items`.
    pub(crate) async fn set_items(&self, agent: AgentId, items: Vec<TodoView>) {
        self.change(agent, |record| record.view.items = items).await;
    }

    /// `agent`'s request works on the plan at `plan` now.
    pub(crate) async fn set_plan(&self, agent: AgentId, plan: &str) {
        self.change(agent, |record| {
            if record.view.parent.is_none() {
                record.view.plan = Some(plan.to_string());
            }
        })
        .await;
    }

    /// A `task` call handed work to a spawned agent: start its task,
    /// under its parent's. Returns the task's id.
    pub(crate) async fn begin_spawn(&self, spawned: Spawned<'_>) -> String {
        self.touch(spawned.parent).await;
        let shown = {
            let mut board = lock(&self.board);
            let parent = board.current.get(&spawned.parent);
            let workspace = parent.map_or(spawned.workspace, |p| p.workspace.clone());
            let (parent_id, plan) = parent.map_or((None, None), |p| {
                (Some(p.record.view.id.clone()), p.record.view.plan.clone())
            });
            let now = now_ms();
            let record = TaskRecord {
                view: TaskView {
                    id: new_id(now),
                    title: first_line(spawned.prompt, TITLE_CHARS),
                    state: TaskState::Running,
                    agent_id: spawned.child.to_hex(),
                    agent_name: spawned.name.to_string(),
                    assignee: Some(spawned.assignee.to_string()),
                    parent: parent_id,
                    plan_todo: spawned.todo_id.filter(|_| plan.is_some()),
                    plan,
                    updated_ms: now,
                    ..TaskView::default()
                },
                created_ms: now,
                tool_use_id: spawned.tool_use_id,
            };
            keep(&workspace, &record);
            let shown = (workspace.clone(), record.view.clone());
            board.current.insert(
                spawned.child,
                Current {
                    workspace,
                    record,
                    saved: true,
                },
            );
            shown
        };
        let id = shown.1.id.clone();
        self.show(shown).await;
        id
    }

    /// The spawned `agent` reported back: `Ok` with what it said, or why
    /// it failed.
    pub(crate) async fn finish_spawn(&self, agent: AgentId, report: Result<String, String>) {
        let (state, said) = match report {
            Ok(text) => (TaskState::Completed, text),
            Err(reason) => (TaskState::Failed, reason),
        };
        self.finish(agent, |record| {
            record.view.state = state;
            record.view.summary = summary_of(&said);
        })
        .await;
    }

    /// `agent`'s turn answering the user ended: close its request as
    /// `state`, with `summary`. A task it was handed is left to
    /// [`Self::finish_spawn`].
    pub(crate) async fn end_request(
        &self,
        agent: AgentId,
        end: impl FnOnce(&[TodoView]) -> TaskState,
        summary: Option<String>,
    ) {
        let is_request = lock(&self.board)
            .current
            .get(&agent)
            .is_some_and(|current| current.record.view.parent.is_none());
        if !is_request {
            return;
        }
        self.finish(agent, |record| {
            record.view.state = end(&record.view.items);
            record.view.summary = summary;
        })
        .await;
    }

    async fn finish(&self, agent: AgentId, finish: impl FnOnce(&mut TaskRecord)) {
        let shown = {
            let mut board = lock(&self.board);
            let Some(mut current) = board.current.remove(&agent) else {
                return;
            };
            if !current.saved {
                return;
            }
            finish(&mut current.record);
            if current.record.view.state != TaskState::Completed {
                close_open_items(&mut current.record.view.items);
            }
            current.record.view.updated_ms = now_ms();
            keep(&current.workspace, &current.record);
            (current.workspace, current.record.view)
        };
        self.show(shown).await;
    }

    /// Every task in `workspace`, for the TASKS tab of `agent`, which
    /// works there. Tasks an earlier run left running are open now.
    pub(crate) async fn send_all(&self, agent: AgentId, workspace: &Path) {
        let views = {
            let mut board = lock(&self.board);
            board.workspaces.insert(agent, workspace.to_path_buf());
            let mut records = load_all(workspace);
            if board.recovered.insert(workspace.to_path_buf()) {
                let live: HashSet<&str> = board
                    .current
                    .values()
                    .map(|current| current.record.view.id.as_str())
                    .collect();
                for record in &mut records {
                    if record.view.state == TaskState::Running
                        && !live.contains(record.view.id.as_str())
                    {
                        record.view.state = TaskState::Open;
                        keep(workspace, record);
                    }
                }
            }
            records.into_iter().map(|record| record.view).collect()
        };
        if let Some((commands, _)) = &self.ui {
            let _ = commands
                .send(UiCommand::ForAgent {
                    agent_id: agent.to_hex(),
                    command: Box::new(UiCommand::SetTasks(views)),
                })
                .await;
        }
    }

    /// Show `view`, kept in `workspace`, when the agent on screen works
    /// there.
    async fn show(&self, (workspace, view): (PathBuf, TaskView)) {
        let Some((commands, shown)) = &self.ui else {
            return;
        };
        let Some(agent) = shown.get() else {
            return;
        };
        let here = lock(&self.board).workspaces.get(&agent) == Some(&workspace);
        if here {
            let _ = commands
                .send(UiCommand::ForAgent {
                    agent_id: agent.to_hex(),
                    command: Box::new(UiCommand::UpsertTask(view)),
                })
                .await;
        }
    }
}

/// A task that stopped short leaves the item it was on in progress no
/// longer; it is what is left to do.
fn close_open_items(items: &mut [TodoView]) {
    for item in items {
        if item.state == TodoState::InProgress {
            item.state = TodoState::Pending;
        }
    }
}

fn keep(workspace: &Path, record: &TaskRecord) {
    if let Err(e) = save(workspace, record) {
        warn!(task = %record.view.id, error = %e, "Could not save the task");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(content: &str, state: TodoState) -> TodoView {
        TodoView {
            content: content.to_string(),
            state,
            assignee: None,
            task: None,
        }
    }

    fn agent(n: u8) -> AgentId {
        AgentId::new([n; 32])
    }

    fn board() -> (TaskBoard, mpsc::Receiver<UiCommand>, RecordPanelAgent) {
        let (tx, rx) = mpsc::channel(64);
        let shown = RecordPanelAgent::default();
        (TaskBoard::new(tx, shown.clone()), rx, shown)
    }

    fn upserts(rx: &mut mpsc::Receiver<UiCommand>) -> Vec<TaskView> {
        let mut out = Vec::new();
        while let Ok(command) = rx.try_recv() {
            if let UiCommand::ForAgent { command, .. } = command {
                if let UiCommand::UpsertTask(view) = *command {
                    out.push(view);
                }
            }
        }
        out
    }

    #[test]
    fn summaries_are_the_first_paragraph_on_one_line() {
        assert_eq!(
            summary_of("Fixed it.\nAll tests pass.\n\nDetails…").as_deref(),
            Some("Fixed it. All tests pass.")
        );
        assert_eq!(summary_of("  \n"), None);
        assert_eq!(first_line("\n  Do the thing  \nmore", 6), "Do th…");
    }

    #[test]
    fn a_request_ends_open_while_items_are_left() {
        let left = [
            item("a", TodoState::Completed),
            item("b", TodoState::Pending),
        ];
        assert_eq!(request_state(false, false, &left), TaskState::Open);
        assert_eq!(
            request_state(false, false, &left[..1]),
            TaskState::Completed
        );
        assert_eq!(request_state(false, true, &left[..1]), TaskState::Failed);
        assert_eq!(request_state(true, true, &left), TaskState::Cancelled);
    }

    #[tokio::test]
    async fn a_request_is_kept_once_its_turn_does_something() {
        let dir = tempfile::tempdir().unwrap();
        let (tasks, mut rx, shown) = board();
        let lead = agent(1);
        shown.set(lead);
        tasks.send_all(lead, dir.path()).await;

        tasks.begin_request(lead, dir.path().to_path_buf(), "Lead", "hi", None);
        tasks
            .end_request(lead, |items| request_state(false, false, items), None)
            .await;
        assert!(load_all(dir.path()).is_empty(), "a turn that only answers");

        tasks.begin_request(
            lead,
            dir.path().to_path_buf(),
            "Lead",
            "Fix the bug\nplease",
            None,
        );
        tasks.touch(lead).await;
        tasks
            .set_items(lead, vec![item("find it", TodoState::InProgress)])
            .await;
        tasks.set_plan(lead, ".aura/plans/fix.plan.md").await;
        tasks
            .end_request(
                lead,
                |items| request_state(false, false, items),
                Some("Found it.".to_string()),
            )
            .await;

        let kept = load_all(dir.path());
        assert_eq!(kept.len(), 1);
        let view = &kept[0].view;
        assert_eq!(view.title, "Fix the bug");
        assert_eq!(view.state, TaskState::Open);
        assert_eq!(view.items[0].state, TodoState::Pending, "left to do");
        assert_eq!(view.plan.as_deref(), Some(".aura/plans/fix.plan.md"));
        assert_eq!(view.summary.as_deref(), Some("Found it."));
        let shown = upserts(&mut rx);
        assert!(shown.len() >= 3);
        assert_eq!(shown.last(), Some(view));
    }

    #[tokio::test]
    async fn a_spawn_is_a_task_under_its_parents_on_its_plans_todo() {
        let dir = tempfile::tempdir().unwrap();
        let (tasks, _rx, _shown) = board();
        let (lead, child) = (agent(1), agent(2));
        let plan = Some(".aura/plans/p.plan.md".to_string());
        tasks.begin_request(lead, dir.path().to_path_buf(), "Lead", "Build it", plan);
        let id = tasks
            .begin_spawn(Spawned {
                parent: lead,
                child,
                workspace: PathBuf::from("/elsewhere"),
                name: "coder #1",
                assignee: "coder",
                prompt: "Write the parser",
                tool_use_id: Some("call-1".to_string()),
                todo_id: Some("parser".to_string()),
            })
            .await;
        tasks.begin_request(child, PathBuf::from("/elsewhere"), "coder #1", "x", None);
        tasks
            .set_items(child, vec![item("lex", TodoState::Completed)])
            .await;
        tasks
            .finish_spawn(child, Ok("Parser written.".to_string()))
            .await;
        tasks
            .end_request(lead, |items| request_state(false, false, items), None)
            .await;

        let kept = load_all(dir.path());
        assert_eq!(kept.len(), 2, "both kept in the parent's workspace");
        let spawned = kept.iter().find(|r| r.view.id == id).unwrap();
        let request = kept.iter().find(|r| r.view.id != id).unwrap();
        assert_eq!(spawned.view.parent.as_ref(), Some(&request.view.id));
        assert_eq!(spawned.view.assignee.as_deref(), Some("coder"));
        assert_eq!(spawned.view.plan_todo.as_deref(), Some("parser"));
        assert_eq!(spawned.view.state, TaskState::Completed);
        assert_eq!(spawned.view.items.len(), 1);
        assert_eq!(spawned.tool_use_id.as_deref(), Some("call-1"));
        assert_eq!(request.view.state, TaskState::Completed);
        assert_eq!(
            plan_tasks(dir.path(), ".aura/plans/p.plan.md").get("parser"),
            Some(&id)
        );
    }

    #[tokio::test]
    async fn tasks_left_running_by_an_earlier_run_open_on_first_show() {
        let dir = tempfile::tempdir().unwrap();
        let stale = TaskRecord {
            view: TaskView {
                id: "old".to_string(),
                state: TaskState::Running,
                ..TaskView::default()
            },
            ..TaskRecord::default()
        };
        save(dir.path(), &stale).unwrap();
        let (tasks, mut rx, _shown) = board();
        tasks.send_all(agent(1), dir.path()).await;
        assert_eq!(load_all(dir.path())[0].view.state, TaskState::Open);
        let Some(UiCommand::ForAgent { command, .. }) = rx.recv().await else {
            panic!("the tasks are sent");
        };
        assert!(
            matches!(*command, UiCommand::SetTasks(views) if views[0].state == TaskState::Open)
        );
    }
}
