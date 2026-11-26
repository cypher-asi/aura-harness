//! The board's state: each agent's current task, the open channel, and tasks set aside.

use super::*;

/// An agent's task while it works on it.
pub(super) struct Current {
    /// Where the task is kept
    pub(super) workspace: PathBuf,
    pub(super) record: TaskRecord,
    /// Whether it is on disk and shown yet: a request is not until its
    /// turn does something
    pub(super) saved: bool,
    /// Whether the end of the agent's turn settles it
    /// ([`TaskBoard::end_request`]): a request, or a task a harness notice
    /// reopened. A task handed over by a `task` call waits for that call
    /// to report instead ([`TaskBoard::finish_spawn`]).
    pub(super) settles_with_turn: bool,
    /// The todo id of each of `record.view.items`, in order. The views
    /// carry no ids, and a spawn names its todo by id.
    pub(super) item_ids: Vec<String>,
    /// For each todo id, the newest task spawned to do it. Kept apart
    /// from the items because every todo update replaces them whole and
    /// the link has to outlive that.
    pub(super) todo_tasks: HashMap<String, String>,
}

impl Current {
    pub(super) fn new(
        workspace: PathBuf,
        record: TaskRecord,
        saved: bool,
        settles_with_turn: bool,
    ) -> Self {
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
    pub(super) fn set_items(&mut self, items: Vec<(String, TodoView)>) {
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
pub(super) fn link_parent(
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
pub(super) struct Board {
    pub(super) current: HashMap<AgentId, Current>,
    /// Each agent's workspace, as last shown
    pub(super) workspaces: HashMap<AgentId, PathBuf>,
    /// Workspaces whose tasks left running by an earlier run were marked
    /// open
    pub(super) recovered: HashSet<PathBuf>,
    /// Workspaces whose tasks are set aside ([`TaskBoard::set_aside`])
    pub(super) aside: HashMap<PathBuf, Aside>,
    /// The channel on screen, whose tasks are shown again as they change
    pub(super) channel: Option<OpenChannel>,
}

/// The channel on screen: its id and the workspace of each member.
pub(super) struct OpenChannel {
    pub(super) id: String,
    pub(super) members: HashMap<AgentId, PathBuf>,
}

impl OpenChannel {
    pub(super) fn has_member(&self, agent_hex: &str) -> bool {
        self.members.keys().any(|agent| agent.to_hex() == agent_hex)
    }
}

/// Every task of the agents in `members`, each a channel member with its
/// workspace, newest first. A workspace several members share is read
/// once; tasks of agents outside the channel are left out.
pub(super) fn channel_tasks(
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
pub(super) fn channel_command(board: &Board, open: &OpenChannel) -> UiCommand {
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
    let members = super::super::channels::member_ids(hub, channel_id).await?;
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
pub(super) struct Aside {
    /// Where they are, and where tasks kept meanwhile go
    pub(super) dir: PathBuf,
    /// How many [`TasksAside`] keep them there
    pub(super) holds: u32,
}

/// The folder `workspace`'s tasks are in, given the workspaces whose tasks
/// are `aside`.
pub(super) fn tasks_dir(aside: &HashMap<PathBuf, Aside>, workspace: &Path) -> PathBuf {
    aside
        .get(workspace)
        .map_or_else(|| workspace.join(TASKS_DIR), |aside| aside.dir.clone())
}

/// Keeps a workspace's tasks out of it until dropped; see
/// [`TaskBoard::set_aside`].
pub(crate) struct TasksAside {
    pub(super) board: Arc<TaskBoard>,
    pub(super) workspace: PathBuf,
}

impl Drop for TasksAside {
    fn drop(&mut self) {
        self.board.put_back(&self.workspace);
    }
}

/// Where `workspace`'s tasks go while set aside: inside its `.git`, which
/// searches leave out, or beside it. `None` when it has neither.
pub(super) fn aside_dir(workspace: &Path) -> Option<PathBuf> {
    let git = workspace.join(".git");
    if git.is_dir() {
        return Some(git.join("aura-tasks-aside"));
    }
    let name = workspace.file_name()?.to_string_lossy();
    Some(workspace.with_file_name(format!(".{name}.aura-tasks")))
}

/// Move every file of the folder `from` into the folder `to`, replacing
/// what has the same name, and remove `from`.
pub(super) fn move_files(from: &Path, to: &Path) -> std::io::Result<()> {
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
