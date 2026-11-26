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
use aura_core_types::AgentId;
use aura_ui_terminal::events::{TaskState, TaskView, TodoState, TodoView};
use aura_ui_terminal::UiCommand;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
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

/// An agent's task while it works on it.
struct Current {
    /// Where the task is kept
    workspace: PathBuf,
    record: TaskRecord,
    /// Whether it is on disk and shown yet: a request is not until its
    /// turn does something
    saved: bool,
    /// Whether the end of the agent's turn settles it
    /// ([`TaskBoard::end_request`]): a request, or a task a harness notice
    /// reopened. A task handed over by a `task` call waits for that call
    /// to report instead ([`TaskBoard::finish_spawn`]).
    settles_with_turn: bool,
    /// The todo id of each of `record.view.items`, in order. The views
    /// carry no ids, and a spawn names its todo by id.
    item_ids: Vec<String>,
    /// For each todo id, the newest task spawned to do it. Kept apart
    /// from the items because every todo update replaces them whole and
    /// the link has to outlive that.
    todo_tasks: HashMap<String, String>,
}

impl Current {
    fn new(workspace: PathBuf, record: TaskRecord, saved: bool, settles_with_turn: bool) -> Self {
        Self {
            workspace,
            record,
            saved,
            settles_with_turn,
            item_ids: Vec::new(),
            todo_tasks: HashMap::new(),
        }
    }

    /// The task `task` does the todo `todo` now, in place of any earlier
    /// one. Returns whether an item already on the list changed.
    fn link_todo(&mut self, todo: &str, task: &str) -> bool {
        self.todo_tasks.insert(todo.to_string(), task.to_string());
        let mut changed = false;
        for (id, item) in self.item_ids.iter().zip(&mut self.record.view.items) {
            if id == todo && item.task.as_deref() != Some(task) {
                item.task = Some(task.to_string());
                changed = true;
            }
        }
        changed
    }

    /// The todo list is now `items`, each with its todo id; an item a
    /// spawned task is doing names it.
    fn set_items(&mut self, items: Vec<(String, TodoView)>) {
        let (ids, mut views): (Vec<String>, Vec<TodoView>) = items.into_iter().unzip();
        for (id, view) in ids.iter().zip(&mut views) {
            if let Some(task) = self.todo_tasks.get(id) {
                view.task = Some(task.clone());
            }
        }
        self.item_ids = ids;
        self.record.view.items = views;
    }
}

/// Link the todo `todo` of `parent`'s current task to the spawned task
/// `task`, saving the parent when an item it shows changed. Returns what
/// to show then.
fn link_parent(
    board: &mut Board,
    parent: AgentId,
    todo: &str,
    task: &str,
    now: u64,
) -> Option<(PathBuf, TaskView)> {
    let current = board.current.get_mut(&parent)?;
    if !current.link_todo(todo, task) {
        return None;
    }
    current.record.view.updated_ms = now;
    current.saved = true;
    keep(
        &tasks_dir(&board.aside, &current.workspace),
        &current.record,
    );
    Some((current.workspace.clone(), current.record.view.clone()))
}

#[derive(Default)]
struct Board {
    current: HashMap<AgentId, Current>,
    /// Each agent's workspace, as last shown
    workspaces: HashMap<AgentId, PathBuf>,
    /// Workspaces whose tasks left running by an earlier run were marked
    /// open
    recovered: HashSet<PathBuf>,
    /// Workspaces whose tasks are set aside ([`TaskBoard::set_aside`])
    aside: HashMap<PathBuf, Aside>,
    /// The channel on screen, whose tasks are shown again as they change
    channel: Option<OpenChannel>,
}

/// The channel on screen: its id and the workspace of each member.
struct OpenChannel {
    id: String,
    members: HashMap<AgentId, PathBuf>,
}

impl OpenChannel {
    fn has_member(&self, agent_hex: &str) -> bool {
        self.members.keys().any(|agent| agent.to_hex() == agent_hex)
    }
}

/// Every task of the agents in `members`, each a channel member with its
/// workspace, newest first. A workspace several members share is read
/// once; tasks of agents outside the channel are left out.
fn channel_tasks(
    aside: &HashMap<PathBuf, Aside>,
    members: &HashMap<AgentId, PathBuf>,
) -> Vec<TaskView> {
    let agents: HashSet<String> = members.keys().map(AgentId::to_hex).collect();
    let workspaces: HashSet<&PathBuf> = members.values().collect();
    let mut seen: HashSet<(&PathBuf, String)> = HashSet::new();
    let mut records: Vec<TaskRecord> = Vec::new();
    for workspace in workspaces {
        for record in load_from(&tasks_dir(aside, workspace)) {
            if agents.contains(&record.view.agent_id)
                && seen.insert((workspace, record.view.id.clone()))
            {
                records.push(record);
            }
        }
    }
    records.sort_by(|a, b| (b.created_ms, &b.view.id).cmp(&(a.created_ms, &a.view.id)));
    records.into_iter().map(|record| record.view).collect()
}

/// The command that shows `open`'s tasks.
fn channel_command(board: &Board, open: &OpenChannel) -> UiCommand {
    UiCommand::SetChannelTasks {
        channel_id: open.id.clone(),
        tasks: channel_tasks(&board.aside, &open.members),
    }
}

/// Show the CHANNEL tab the tasks of every member of channel
/// `channel_id`, and keep showing them as they change until the channel
/// is closed ([`TaskBoard::close_channel`]). A member works in its own
/// folder, else the one of the agents that spawned it or its project's,
/// else the workspace of its running kernel; a member with none has no
/// tasks to show.
///
/// # Errors
/// Returns an error if the channel's members cannot be read.
pub(crate) async fn send_channel(hub: &AgentHub, channel_id: &str) -> anyhow::Result<()> {
    let members = super::channels::member_ids(hub, channel_id).await?;
    let (mut workspaces, missing): (HashMap<AgentId, PathBuf>, Vec<AgentId>) = {
        let registry = lock(&hub.registry);
        let mut found = HashMap::new();
        let mut missing = Vec::new();
        for agent in members {
            match registry.project_folder(agent) {
                Some(folder) => {
                    found.insert(agent, folder);
                }
                None => missing.push(agent),
            }
        }
        (found, missing)
    };
    for agent in missing {
        if let Some(kernel) = hub.running_kernel(agent) {
            workspaces.insert(agent, kernel.agent_workspace());
        }
    }
    hub.factory
        .tasks
        .open_channel(channel_id.to_string(), workspaces)
        .await;
    Ok(())
}

/// A workspace's tasks while they are set aside.
struct Aside {
    /// Where they are, and where tasks kept meanwhile go
    dir: PathBuf,
    /// How many [`TasksAside`] keep them there
    holds: u32,
}

/// The folder `workspace`'s tasks are in, given the workspaces whose tasks
/// are `aside`.
fn tasks_dir(aside: &HashMap<PathBuf, Aside>, workspace: &Path) -> PathBuf {
    aside
        .get(workspace)
        .map_or_else(|| workspace.join(TASKS_DIR), |aside| aside.dir.clone())
}

/// Keeps a workspace's tasks out of it until dropped; see
/// [`TaskBoard::set_aside`].
pub(crate) struct TasksAside {
    board: Arc<TaskBoard>,
    workspace: PathBuf,
}

impl Drop for TasksAside {
    fn drop(&mut self) {
        self.board.put_back(&self.workspace);
    }
}

/// Where `workspace`'s tasks go while set aside: inside its `.git`, which
/// searches leave out, or beside it. `None` when it has neither.
fn aside_dir(workspace: &Path) -> Option<PathBuf> {
    let git = workspace.join(".git");
    if git.is_dir() {
        return Some(git.join("aura-tasks-aside"));
    }
    let name = workspace.file_name()?.to_string_lossy();
    Some(workspace.with_file_name(format!(".{name}.aura-tasks")))
}

/// Move every file of the folder `from` into the folder `to`, replacing
/// what has the same name, and remove `from`.
fn move_files(from: &Path, to: &Path) -> std::io::Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    if !to.exists() && std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        std::fs::rename(entry.path(), to.join(entry.file_name()))?;
    }
    std::fs::remove_dir(from)
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
    /// `workspace`, working on the plan at `plan`; `builds` when the
    /// request is the one that builds that plan. Nothing is kept until
    /// the turn does something ([`Self::touch`]). An agent doing a task
    /// it was handed keeps that one.
    pub(crate) fn begin_request(
        &self,
        agent: AgentId,
        workspace: PathBuf,
        name: &str,
        prompt: &str,
        plan: Option<String>,
        builds: bool,
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
                builds: builds && plan.is_some(),
                plan,
                updated_ms: now,
                ..TaskView::default()
            },
            created_ms: now,
            tool_use_id: None,
        };
        board
            .current
            .insert(agent, Current::new(workspace, record, false, true));
    }

    /// A harness notice (a background job or agent ending) woke `agent`
    /// in `workspace`. The notice is about work the agent already has, so
    /// it starts no task: the agent's latest task there runs again, a
    /// request or a task it was handed, and its turn settles it as it
    /// would a request's. A task the agent is still doing stays as it
    /// is; an agent with no task gets none.
    pub(crate) async fn resume_request(&self, agent: AgentId, workspace: &Path) {
        let shown = {
            let board = &mut *lock(&self.board);
            if board.current.contains_key(&agent) {
                return;
            }
            let dir = tasks_dir(&board.aside, workspace);
            let hex = agent.to_hex();
            let Some(mut record) = load_from(&dir)
                .into_iter()
                .find(|record| record.view.agent_id == hex)
            else {
                return;
            };
            record.view.state = TaskState::Running;
            record.view.updated_ms = now_ms();
            keep(&dir, &record);
            let shown = (workspace.to_path_buf(), record.view.clone());
            board.current.insert(
                agent,
                Current::new(workspace.to_path_buf(), record, true, true),
            );
            shown
        };
        self.show(shown).await;
    }

    /// `agent`'s turn did something: keep and show its task.
    pub(crate) async fn touch(&self, agent: AgentId) {
        self.change(agent, |_| {}).await;
    }

    /// Change `agent`'s task with `change`, then keep and show it.
    pub(crate) async fn change(&self, agent: AgentId, change: impl FnOnce(&mut TaskRecord)) {
        self.change_current(agent, |current| change(&mut current.record))
            .await;
    }

    /// Change `agent`'s current entry with `change`, then keep and show
    /// its task if that changed or was not kept yet.
    async fn change_current(&self, agent: AgentId, change: impl FnOnce(&mut Current)) {
        let shown = {
            let board = &mut *lock(&self.board);
            let Some(current) = board.current.get_mut(&agent) else {
                return;
            };
            let before = current.record.clone();
            change(current);
            if current.saved && current.record == before {
                return;
            }
            current.record.view.updated_ms = now_ms();
            current.saved = true;
            keep(
                &tasks_dir(&board.aside, &current.workspace),
                &current.record,
            );
            (current.workspace.clone(), current.record.view.clone())
        };
        self.show(shown).await;
    }

    /// `agent`'s todo list is now `items`, each with its todo id, which
    /// links an item to the task spawned to do it ([`Self::begin_spawn`]).
    pub(crate) async fn set_items(&self, agent: AgentId, items: Vec<(String, TodoView)>) {
        self.change_current(agent, |current| current.set_items(items))
            .await;
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
    /// under its parent's. A spawn for one of the parent's todos links
    /// that item to it at once, so the link shows before the next todo
    /// update. Returns the task's id.
    pub(crate) async fn begin_spawn(&self, spawned: Spawned<'_>) -> String {
        self.touch(spawned.parent).await;
        let (shown, parent_shown) = {
            let board = &mut *lock(&self.board);
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
                    plan_todo: plan.as_ref().and(spawned.todo_id.clone()),
                    plan,
                    updated_ms: now,
                    ..TaskView::default()
                },
                created_ms: now,
                tool_use_id: spawned.tool_use_id,
            };
            keep(&tasks_dir(&board.aside, &workspace), &record);
            let parent_shown = spawned
                .todo_id
                .as_deref()
                .and_then(|todo| link_parent(board, spawned.parent, todo, &record.view.id, now));
            let shown = (workspace.clone(), record.view.clone());
            board
                .current
                .insert(spawned.child, Current::new(workspace, record, true, false));
            (shown, parent_shown)
        };
        let id = shown.1.id.clone();
        // The spawned task first, so the item never names a task not shown.
        self.show(shown).await;
        if let Some(parent_shown) = parent_shown {
            self.show(parent_shown).await;
        }
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

    /// `agent`'s turn ended: close its request, or the task a harness
    /// notice reopened, as `state`, with `summary` when the turn said
    /// something. A task it was handed is left to [`Self::finish_spawn`].
    pub(crate) async fn end_request(
        &self,
        agent: AgentId,
        end: impl FnOnce(&[TodoView]) -> TaskState,
        summary: Option<String>,
    ) {
        let settles = lock(&self.board)
            .current
            .get(&agent)
            .is_some_and(|current| current.settles_with_turn);
        if !settles {
            return;
        }
        self.finish(agent, |record| {
            record.view.state = end(&record.view.items);
            // A reopened task that only answered keeps what it said before.
            if summary.is_some() {
                record.view.summary = summary;
            }
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
            keep(
                &tasks_dir(&board.aside, &current.workspace),
                &current.record,
            );
            (current.workspace, current.record.view)
        };
        self.show(shown).await;
    }

    /// [`Self::all_in`], shown in `agent`'s TASKS tab.
    #[cfg(test)]
    pub(crate) async fn send_all(&self, agent: AgentId, workspace: &Path) {
        let views = self.all_in(agent, workspace);
        self.send_tasks(agent, views).await;
    }

    /// Every task in `workspace`, for the TASKS tab of `agent`, which
    /// works there. Tasks an earlier run left running are open now. Reads
    /// every task file, so it is called off the event loop.
    pub(crate) fn all_in(&self, agent: AgentId, workspace: &Path) -> Vec<TaskView> {
        let mut board = lock(&self.board);
        board.workspaces.insert(agent, workspace.to_path_buf());
        let dir = tasks_dir(&board.aside, workspace);
        let mut records = load_from(&dir);
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
                    keep(&dir, record);
                }
            }
        }
        records.into_iter().map(|record| record.view).collect()
    }

    /// Show `agent` the tasks `views` in its TASKS tab.
    pub(crate) async fn send_tasks(&self, agent: AgentId, views: Vec<TaskView>) {
        if let Some((commands, _)) = &self.ui {
            let _ = commands
                .send(UiCommand::ForAgent {
                    agent_id: agent.to_hex(),
                    command: Box::new(UiCommand::SetTasks(views)),
                })
                .await;
        }
    }

    /// Move `workspace`'s tasks out of it until the returned guard drops,
    /// keeping the tasks saved meanwhile with them: they quote the
    /// prompts, so a check that searches the workspace would find them.
    pub(crate) fn set_aside(self: &Arc<Self>, workspace: &Path) -> TasksAside {
        let mut board = lock(&self.board);
        if let Some(aside) = board.aside.get_mut(workspace) {
            aside.holds += 1;
        } else {
            let records = workspace.join(TASKS_DIR);
            let dir = aside_dir(workspace)
                .and_then(|dir| match move_files(&records, &dir) {
                    Ok(()) => Some(dir),
                    Err(e) => {
                        warn!(error = %e, from = %records.display(), "Could not set the tasks aside");
                        None
                    }
                })
                .unwrap_or(records);
            board
                .aside
                .insert(workspace.to_path_buf(), Aside { dir, holds: 1 });
        }
        TasksAside {
            board: Arc::clone(self),
            workspace: workspace.to_path_buf(),
        }
    }

    /// One [`TasksAside`] of `workspace` is done: the last one puts its
    /// tasks back.
    fn put_back(&self, workspace: &Path) {
        let mut board = lock(&self.board);
        let Some(aside) = board.aside.get_mut(workspace) else {
            return;
        };
        aside.holds -= 1;
        if aside.holds > 0 {
            return;
        }
        let Some(aside) = board.aside.remove(workspace) else {
            return;
        };
        let records = workspace.join(TASKS_DIR);
        if aside.dir != records {
            if let Err(e) = move_files(&aside.dir, &records) {
                warn!(error = %e, from = %aside.dir.display(), "Could not put the tasks back");
            }
        }
    }

    /// Show the tasks of the channel `id`, whose members work in
    /// `members`, and show them again whenever one of them changes.
    pub(crate) async fn open_channel(&self, id: String, members: HashMap<AgentId, PathBuf>) {
        let command = {
            let mut board = lock(&self.board);
            let open = OpenChannel { id, members };
            let command = channel_command(&board, &open);
            board.channel = Some(open);
            command
        };
        self.send(command).await;
    }

    /// No channel is on screen any more.
    pub(crate) fn close_channel(&self) {
        lock(&self.board).channel = None;
    }

    /// `agent`, working in `workspace`, joined channel `channel_id`: when
    /// that channel is on screen, its tasks count from now on.
    pub(crate) fn join_channel(&self, channel_id: &str, agent: AgentId, workspace: PathBuf) {
        if let Some(open) = lock(&self.board).channel.as_mut() {
            if open.id == channel_id {
                open.members.insert(agent, workspace);
            }
        }
    }

    /// Show the tasks of the channel on screen again when the agent with
    /// hex id `agent_hex` is one of its members.
    async fn refresh_channel(&self, agent_hex: &str) {
        let command = {
            let board = lock(&self.board);
            match board.channel.as_ref() {
                Some(open) if open.has_member(agent_hex) => channel_command(&board, open),
                _ => return,
            }
        };
        self.send(command).await;
    }

    async fn send(&self, command: UiCommand) {
        if let Some((commands, _)) = &self.ui {
            let _ = commands.send(command).await;
        }
    }

    /// Show `view`, kept in `workspace`, when the agent on screen works
    /// there, and in the channel on screen when its agent is a member.
    async fn show(&self, (workspace, view): (PathBuf, TaskView)) {
        self.refresh_channel(&view.agent_id).await;
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

/// Save `record` into the folder `dir`.
fn keep(dir: &Path, record: &TaskRecord) {
    if let Err(e) = save_in(dir, record) {
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

    fn todo(id: &str, content: &str, state: TodoState) -> (String, TodoView) {
        (id.to_string(), item(content, state))
    }

    /// Two todos, `a` and `b`, as the lead's list gives them.
    fn two_todos() -> Vec<(String, TodoView)> {
        vec![
            todo("a", "find it", TodoState::InProgress),
            todo("b", "fix it", TodoState::Pending),
        ]
    }

    /// A spawn by `parent` of `child`, for the todo `todo_id`.
    fn spawn_of(
        parent: AgentId,
        child: AgentId,
        workspace: &Path,
        todo_id: Option<&str>,
    ) -> Spawned<'static> {
        Spawned {
            parent,
            child,
            workspace: workspace.to_path_buf(),
            name: "coder #1",
            assignee: "coder",
            prompt: "Fix it",
            tool_use_id: None,
            todo_id: todo_id.map(str::to_string),
        }
    }

    /// The task each of the request's items names, from disk.
    fn request_item_tasks(workspace: &Path) -> Vec<Option<String>> {
        load_all(workspace)
            .into_iter()
            .find(|record| record.view.parent.is_none())
            .map(|record| record.view.items.into_iter().map(|i| i.task).collect())
            .unwrap_or_default()
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

        tasks.begin_request(lead, dir.path().to_path_buf(), "Lead", "hi", None, false);
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
            false,
        );
        tasks.touch(lead).await;
        tasks
            .set_items(lead, vec![todo("1", "find it", TodoState::InProgress)])
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
        tasks.begin_request(
            lead,
            dir.path().to_path_buf(),
            "Lead",
            "Build it",
            plan,
            true,
        );
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
        tasks.begin_request(
            child,
            PathBuf::from("/elsewhere"),
            "coder #1",
            "x",
            None,
            false,
        );
        tasks
            .set_items(child, vec![todo("1", "lex", TodoState::Completed)])
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
        assert!(request.view.builds, "the request that builds the plan");
        assert!(!spawned.view.builds);
        assert_eq!(
            PlanTasks::load(dir.path()).task(".aura/plans/p.plan.md", "parser"),
            Some(id.as_str())
        );
    }

    #[tokio::test]
    async fn a_spawn_for_a_todo_links_the_item_already_on_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let (tasks, mut rx, shown) = board();
        let (lead, child) = (agent(1), agent(2));
        shown.set(lead);
        tasks.send_all(lead, dir.path()).await;
        tasks.begin_request(lead, dir.path().to_path_buf(), "Lead", "Fix", None, false);
        tasks.set_items(lead, two_todos()).await;
        upserts(&mut rx);

        let id = tasks
            .begin_spawn(spawn_of(lead, child, dir.path(), Some("b")))
            .await;

        assert_eq!(request_item_tasks(dir.path()), vec![None, Some(id.clone())]);
        let shown = upserts(&mut rx);
        let last = shown.last().unwrap();
        assert!(last.parent.is_none(), "the parent is shown after the spawn");
        assert_eq!(last.items[1].task.as_ref(), Some(&id));
        assert!(shown.iter().any(|view| view.id == id));
    }

    #[tokio::test]
    async fn a_spawn_for_a_todo_links_the_item_when_the_list_comes_later() {
        let dir = tempfile::tempdir().unwrap();
        let (tasks, _rx, _shown) = board();
        let (lead, child) = (agent(1), agent(2));
        tasks.begin_request(lead, dir.path().to_path_buf(), "Lead", "Fix", None, false);
        let id = tasks
            .begin_spawn(spawn_of(lead, child, dir.path(), Some("b")))
            .await;

        tasks.set_items(lead, two_todos()).await;
        assert_eq!(request_item_tasks(dir.path()), vec![None, Some(id.clone())]);

        let mut done = two_todos();
        done[1].1.state = TodoState::Completed;
        tasks.set_items(lead, done).await;
        assert_eq!(
            request_item_tasks(dir.path()),
            vec![None, Some(id)],
            "the link outlives the list it was made on"
        );
    }

    #[tokio::test]
    async fn a_spawn_without_a_todo_links_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (tasks, _rx, _shown) = board();
        let (lead, child) = (agent(1), agent(2));
        tasks.begin_request(lead, dir.path().to_path_buf(), "Lead", "Fix", None, false);
        tasks.set_items(lead, two_todos()).await;
        tasks
            .begin_spawn(spawn_of(lead, child, dir.path(), None))
            .await;
        assert_eq!(request_item_tasks(dir.path()), vec![None, None]);

        tasks.set_items(lead, two_todos()).await;
        assert_eq!(request_item_tasks(dir.path()), vec![None, None]);
    }

    #[tokio::test]
    async fn a_second_spawn_for_a_todo_replaces_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let (tasks, _rx, _shown) = board();
        let (lead, first, second) = (agent(1), agent(2), agent(3));
        tasks.begin_request(lead, dir.path().to_path_buf(), "Lead", "Fix", None, false);
        tasks.set_items(lead, two_todos()).await;
        tasks
            .begin_spawn(spawn_of(lead, first, dir.path(), Some("b")))
            .await;
        let newer = tasks
            .begin_spawn(spawn_of(lead, second, dir.path(), Some("b")))
            .await;
        assert_eq!(
            request_item_tasks(dir.path()),
            vec![None, Some(newer.clone())]
        );

        tasks.set_items(lead, two_todos()).await;
        assert_eq!(request_item_tasks(dir.path()), vec![None, Some(newer)]);
    }

    #[tokio::test]
    async fn a_notice_after_a_finished_request_reopens_and_settles_it() {
        let dir = tempfile::tempdir().unwrap();
        let (tasks, mut rx, shown) = board();
        let lead = agent(1);
        shown.set(lead);
        tasks.send_all(lead, dir.path()).await;
        tasks.begin_request(
            lead,
            dir.path().to_path_buf(),
            "Lead",
            "Run the slow tests",
            None,
            false,
        );
        tasks.touch(lead).await;
        tasks
            .set_items(lead, vec![todo("1", "run them", TodoState::InProgress)])
            .await;
        tasks
            .end_request(
                lead,
                |items| request_state(false, false, items),
                Some("Started them.".to_string()),
            )
            .await;
        let id = load_all(dir.path())[0].view.id.clone();
        upserts(&mut rx);

        tasks.resume_request(lead, dir.path()).await;
        let kept = load_all(dir.path());
        assert_eq!(kept.len(), 1, "a notice starts no task");
        assert_eq!(kept[0].view.state, TaskState::Running);
        assert_eq!(
            upserts(&mut rx).last().map(|v| v.state),
            Some(TaskState::Running)
        );

        tasks
            .set_items(lead, vec![todo("1", "run them", TodoState::Completed)])
            .await;
        tasks
            .end_request(
                lead,
                |items| request_state(false, false, items),
                Some("They pass.".to_string()),
            )
            .await;
        let kept = load_all(dir.path());
        assert_eq!(kept.len(), 1);
        let view = &kept[0].view;
        assert_eq!(view.id, id);
        assert_eq!(view.title, "Run the slow tests");
        assert_eq!(view.state, TaskState::Completed);
        assert_eq!(view.summary.as_deref(), Some("They pass."));

        tasks.resume_request(lead, dir.path()).await;
        tasks
            .end_request(lead, |items| request_state(false, false, items), None)
            .await;
        let view = &load_all(dir.path())[0].view;
        assert_eq!(view.state, TaskState::Completed);
        assert_eq!(
            view.summary.as_deref(),
            Some("They pass."),
            "a turn that said nothing keeps the summary"
        );
    }

    #[tokio::test]
    async fn a_notice_to_an_agent_with_no_tasks_records_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (tasks, mut rx, shown) = board();
        let (lead, other) = (agent(1), agent(2));
        shown.set(other);
        tasks.send_all(other, dir.path()).await;
        tasks.begin_request(
            lead,
            dir.path().to_path_buf(),
            "Lead",
            "Fix it",
            None,
            false,
        );
        tasks.touch(lead).await;
        tasks
            .end_request(lead, |items| request_state(false, false, items), None)
            .await;
        upserts(&mut rx);

        tasks.resume_request(other, dir.path()).await;
        tasks
            .end_request(other, |items| request_state(false, false, items), None)
            .await;

        let kept = load_all(dir.path());
        assert_eq!(kept.len(), 1, "only the other agent's request");
        assert_eq!(kept[0].view.agent_id, lead.to_hex());
        assert_eq!(kept[0].view.state, TaskState::Completed);
        assert!(upserts(&mut rx).is_empty());
    }

    #[tokio::test]
    async fn a_notice_to_a_finished_spawn_reopens_its_task_under_its_parent() {
        let dir = tempfile::tempdir().unwrap();
        let (tasks, _rx, _shown) = board();
        let (lead, reviewer) = (agent(1), agent(2));
        tasks.begin_request(
            lead,
            dir.path().to_path_buf(),
            "Lead",
            "Review it",
            None,
            false,
        );
        let id = tasks
            .begin_spawn(Spawned {
                parent: lead,
                child: reviewer,
                workspace: dir.path().to_path_buf(),
                name: "reviewer #56",
                assignee: "reviewer",
                prompt: "Review the parser",
                tool_use_id: None,
                todo_id: None,
            })
            .await;
        tasks
            .finish_spawn(reviewer, Err("stopped".to_string()))
            .await;
        tasks
            .end_request(lead, |items| request_state(false, false, items), None)
            .await;

        tasks.resume_request(reviewer, dir.path()).await;
        tasks
            .end_request(
                reviewer,
                |items| request_state(false, false, items),
                Some("Reviewed.".to_string()),
            )
            .await;

        let kept = load_all(dir.path());
        assert_eq!(kept.len(), 2, "no task of its own for the notice");
        let spawned = kept.iter().find(|r| r.view.id == id).unwrap();
        assert!(spawned.view.parent.is_some());
        assert_eq!(spawned.view.state, TaskState::Completed);
        assert_eq!(spawned.view.summary.as_deref(), Some("Reviewed."));
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
        save_in(&dir.path().join(TASKS_DIR), &stale).unwrap();
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

    #[tokio::test]
    async fn tasks_set_aside_leave_the_workspace_and_keep_what_is_saved_meanwhile() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("copy");
        std::fs::create_dir(&workspace).unwrap();
        let (tasks, _rx, _shown) = board();
        let tasks = Arc::new(tasks);
        let (lead, child) = (agent(1), agent(2));
        tasks.begin_request(
            lead,
            workspace.clone(),
            "Lead",
            "Rename calc_total",
            None,
            false,
        );
        tasks.touch(lead).await;

        let first = tasks.set_aside(&workspace);
        let second = tasks.set_aside(&workspace);
        assert!(!workspace.join(TASKS_DIR).exists());
        let aside = root.path().join(".copy.aura-tasks");
        assert_eq!(load_from(&aside).len(), 1);
        tasks
            .begin_spawn(Spawned {
                parent: lead,
                child,
                workspace: workspace.clone(),
                name: "coder #1",
                assignee: "coder",
                prompt: "Rename calc_total in a.rs",
                tool_use_id: None,
                todo_id: None,
            })
            .await;
        assert!(!workspace.join(TASKS_DIR).exists(), "saved aside meanwhile");

        drop(first);
        assert!(
            !workspace.join(TASKS_DIR).exists(),
            "until the last is done"
        );
        drop(second);
        assert_eq!(load_all(&workspace).len(), 2);
        assert!(!aside.exists());
    }

    #[test]
    fn tasks_go_aside_inside_git_when_the_workspace_has_it() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            aside_dir(dir.path()),
            dir.path().parent().map(|p| p.join(format!(
                ".{}.aura-tasks",
                dir.path().file_name().unwrap().to_string_lossy()
            )))
        );
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert_eq!(
            aside_dir(dir.path()),
            Some(dir.path().join(".git/aura-tasks-aside"))
        );
    }
}
