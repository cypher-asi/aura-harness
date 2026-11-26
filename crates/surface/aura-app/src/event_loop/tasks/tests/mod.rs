use super::*;

mod notices;

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

/// A lead's request in `workspace` on a plan that spawns a coder for the
/// plan's todo `parser`; the coder finishes, then the request. Returns the
/// spawned task's id.
async fn spawn_under_plan(workspace: &Path) -> String {
    let (tasks, _rx, _shown) = board();
    let (lead, child) = (agent(1), agent(2));
    let plan = Some(".aura/plans/p.plan.md".to_string());
    tasks.begin_request(
        lead,
        workspace.to_path_buf(),
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
    id
}

#[tokio::test]
async fn a_spawn_is_a_task_under_its_parents_on_its_plans_todo() {
    let dir = tempfile::tempdir().unwrap();
    let id = spawn_under_plan(dir.path()).await;

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
