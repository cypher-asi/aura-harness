//! Harness notices reopening tasks, and tasks set aside.

use super::*;

/// Show `lead`'s tasks in `workspace`, then run a request there that
/// does something and finishes with an item still in progress.
async fn finish_a_request(tasks: &TaskBoard, lead: AgentId, workspace: &Path) {
    tasks.send_all(lead, workspace).await;
    tasks.begin_request(
        lead,
        workspace.to_path_buf(),
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
}

#[tokio::test]
async fn a_notice_after_a_finished_request_reopens_and_settles_it() {
    let dir = tempfile::tempdir().unwrap();
    let (tasks, mut rx, shown) = board();
    let lead = agent(1);
    shown.set(lead);
    finish_a_request(&tasks, lead, dir.path()).await;
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
    assert!(matches!(*command, UiCommand::SetTasks(views) if views[0].state == TaskState::Open));
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
