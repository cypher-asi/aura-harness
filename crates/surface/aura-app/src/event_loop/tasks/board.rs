//! The task board: requests, spawns and notices moving tasks on.

use super::*;

/// Every agent's current task, kept on disk as it moves on and shown in
/// the TASKS tab of the agent on screen when it is in that agent's
/// workspace.
#[derive(Default)]
pub struct TaskBoard {
    pub(super) ui: Option<(mpsc::Sender<UiCommand>, RecordPanelAgent)>,
    pub(super) board: Mutex<Board>,
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
    pub(super) fn put_back(&self, workspace: &Path) {
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
        self.send_if_here(&workspace, UiCommand::UpsertTask(view))
            .await;
    }

    /// Show `view`, a plan in `workspace` just written or moved on, in
    /// the TASKS tab of the agent on screen when it works there, whichever
    /// agent wrote it: a member planning in the folder of the channel on
    /// screen shows in the channel's TASKS at once.
    pub(crate) async fn show_plan(&self, workspace: &Path, view: PlanView) {
        self.send_if_here(workspace, UiCommand::UpsertPlan(view))
            .await;
    }

    /// Send `command` to the TASKS tab of the agent on screen when it
    /// works in `workspace`.
    async fn send_if_here(&self, workspace: &Path, command: UiCommand) {
        let Some((commands, shown)) = &self.ui else {
            return;
        };
        let Some(agent) = shown.get() else {
            return;
        };
        let here = lock(&self.board)
            .workspaces
            .get(&agent)
            .is_some_and(|at| at == workspace);
        if here {
            let _ = commands
                .send(UiCommand::ForAgent {
                    agent_id: agent.to_hex(),
                    command: Box::new(command),
                })
                .await;
        }
    }
}

/// A task that stopped short leaves the item it was on in progress no
/// longer; it is what is left to do.
pub(super) fn close_open_items(items: &mut [TodoView]) {
    for item in items {
        if item.state == TodoState::InProgress {
            item.state = TodoState::Pending;
        }
    }
}

/// Save `record` into the folder `dir`.
pub(super) fn keep(dir: &Path, record: &TaskRecord) {
    if let Err(e) = save_in(dir, record) {
        warn!(task = %record.view.id, error = %e, "Could not save the task");
    }
}
