use super::super::tasks::{TaskRecord, TASKS_DIR};
use super::*;
use aura_ui::events::TaskView;
use std::time::Duration;

fn write_plan(workspace: &Path, file: &str, name: &str, age_secs: u64) {
    let path = workspace.join(PLANS_DIR).join(file);
    let text = format!(
        "---\nname: {name}\ntodos:\n  - id: a\n    content: First\n    status: completed\n---\n"
    );
    std::fs::write(&path, text).unwrap();
    let changed = SystemTime::now() - Duration::from_secs(age_secs);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(changed)
        .unwrap();
}

#[test]
fn plan_views_lists_readable_plans_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    assert!(plan_views(dir.path()).is_empty());

    std::fs::create_dir_all(dir.path().join(PLANS_DIR)).unwrap();
    write_plan(dir.path(), "old_00000001.plan.md", "Old", 300);
    write_plan(dir.path(), "new_00000002.plan.md", "New", 10);
    std::fs::write(dir.path().join(PLANS_DIR).join("notes.md"), "x").unwrap();
    std::fs::write(
        dir.path().join(PLANS_DIR).join("broken_00000003.plan.md"),
        "no frontmatter",
    )
    .unwrap();

    let plans = plan_views(dir.path());
    let names: Vec<&str> = plans.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["New", "Old"]);
    assert_eq!(plans[0].path, ".aura/plans/new_00000002.plan.md");
    assert_eq!(plans[0].todos[0].state, TodoState::Completed);
}

/// Write 20 plans of the given todos into `workspace`'s plans dir.
fn write_perf_plans(workspace: &Path, todo_ids: &[String]) {
    for plan in 0..20 {
        let todos: String = todo_ids
            .iter()
            .map(|id| format!("  - id: {id}\n    content: Do {id}\n    status: pending\n"))
            .collect();
        let text = format!("---\nname: Plan {plan}\ntodos:\n{todos}---\n\nBody.\n");
        let file = format!("plan-{plan}_{plan:08x}.plan.md");
        std::fs::write(workspace.join(PLANS_DIR).join(file), text).unwrap();
    }
}

/// The task record `task` of the load test: one of the 20 plans' todos,
/// with every todo listed as completed.
fn perf_task_record(task: u64, todo_ids: &[String], words: &str) -> TaskRecord {
    let plan = task % 20;
    TaskRecord {
        view: TaskView {
            id: format!("{task:016x}"),
            title: format!("Task {task}"),
            plan: Some(format!("{PLANS_DIR}/plan-{plan}_{plan:08x}.plan.md")),
            plan_todo: Some(todo_ids[usize::try_from(task / 20 % 8).unwrap()].clone()),
            items: todo_ids
                .iter()
                .map(|id| TodoView {
                    content: format!("{id}: {words}"),
                    state: TodoState::Completed,
                    assignee: None,
                    task: None,
                })
                .collect(),
            summary: Some(words.to_string()),
            ..TaskView::default()
        },
        created_ms: task,
        tool_use_id: None,
    }
}

/// Write 600 task records of about 4 KB each into `workspace`'s tasks dir.
fn write_perf_tasks(workspace: &Path, todo_ids: &[String]) {
    let words = "Read the record once and derive the rest from it. ".repeat(8);
    for task in 0..600u64 {
        let record = perf_task_record(task, todo_ids, &words);
        let json = serde_json::to_vec_pretty(&record).unwrap();
        let path = workspace.join(TASKS_DIR).join(format!("{task:016x}.json"));
        std::fs::write(path, json).unwrap();
    }
}

/// How long the Plans tab takes to read in a workspace whose agents
/// built plans for a while: 20 plans of 8 todos, and 600 tasks of
/// about 4 KB each on disk, as `.aura` holds after weeks of use.
///
/// ```text
/// cargo test -p aura-app --lib plan_views_perf -- --ignored --nocapture
/// ```
#[test]
#[ignore = "load test; run with --ignored --nocapture"]
fn plan_views_perf() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(PLANS_DIR)).unwrap();
    std::fs::create_dir_all(dir.path().join(TASKS_DIR)).unwrap();
    let todo_ids: Vec<String> = (0..8).map(|t| format!("todo-{t}")).collect();
    write_perf_plans(dir.path(), &todo_ids);
    write_perf_tasks(dir.path(), &todo_ids);

    let start = std::time::Instant::now();
    let plans = plan_views(dir.path());
    let took = start.elapsed();
    println!(
        "plan_views over 20 plans and 600 tasks: {:.1} ms",
        took.as_secs_f64() * 1000.0
    );
    assert_eq!(plans.len(), 20);
    assert!(plans
        .iter()
        .all(|plan| plan.todos.iter().all(|todo| todo.task.is_some())));
}

#[test]
fn plans_read_on_a_machine_are_viewed_from_their_text() {
    let dir = tempfile::tempdir().unwrap();
    let plan = |name: &str, status: &str| {
        format!(
            "---\nname: {name}\ntodos:\n  - id: a\n    content: First\n    status: {status}\n---\n"
        )
    };
    let open = ".aura/plans/open_00000001.plan.md";
    let done = ".aura/plans/done_00000002.plan.md";
    let plans = [
        (open.to_string(), plan("Open", "in_progress")),
        (done.to_string(), plan("Done", "completed")),
        (
            ".aura/plans/broken_00000003.plan.md".to_string(),
            "no frontmatter".to_string(),
        ),
        (".aura/plans/notes.md".to_string(), plan("Notes", "pending")),
    ];

    let (views, strip) = machine_plan_views(dir.path(), &plans, Some(open));
    let names: Vec<&str> = views.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["Open", "Done"]);
    assert_eq!(views[0].todos[0].state, TodoState::InProgress);
    assert_eq!(strip.map(|v| v.path), Some(open.to_string()));

    let (_, strip) = machine_plan_views(dir.path(), &plans, Some(done));
    assert!(strip.is_none(), "a finished plan is not the agent's plan");
    let (_, strip) = machine_plan_views(dir.path(), &plans, None);
    assert!(strip.is_none());
}

#[test]
fn only_a_bare_build_phrase_typed_with_a_plan_to_build_builds_it() {
    let plan = Some(".aura/plans/p_00000001.plan.md");
    let typed = |text: &str| Prompt::typed(text.to_string(), Vec::new());
    for text in [
        "impl",
        "Implement",
        "  implement it.  ",
        "Implement the plan!",
        "implement plan",
        "BUILD",
        "build it!!",
        "build the plan.",
    ] {
        assert!(asks_to_build(&typed(text), plan, false), "{text:?}");
    }
    for text in [
        "implement the parser",
        "build it and commit",
        "implement?",
        "commit",
        "keep going",
        "",
    ] {
        assert!(!asks_to_build(&typed(text), plan, false), "{text:?}");
    }
    assert!(!asks_to_build(&typed("implement"), None, false), "no plan");
    assert!(
        !asks_to_build(&typed("implement"), plan, true),
        "already building it"
    );
    assert!(
        !asks_to_build(&Prompt::notice("implement".to_string()), plan, false),
        "a harness notice"
    );
    assert!(
        !asks_to_build(&Prompt::plain("implement".to_string()), plan, false),
        "written by the harness"
    );
}

#[test]
fn a_plan_is_open_while_a_todo_is_pending_or_in_progress() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(PLANS_DIR)).unwrap();
    let path = ".aura/plans/p_00000001.plan.md";
    let write = |second: &str| {
        let text = format!(
            "---\nname: P\ntodos:\n  - id: a\n    content: A\n    status: completed\n  \
             - id: b\n    content: B\n    status: {second}\n---\n"
        );
        std::fs::write(dir.path().join(path), text).unwrap();
    };

    assert!(open_plan(dir.path(), None).is_none(), "no plan");
    assert!(open_plan(dir.path(), Some(path)).is_none(), "no plan file");
    assert!(
        open_plan(dir.path(), Some("notes/p.plan.md")).is_none(),
        "not a plan path"
    );
    for closed in ["completed", "cancelled"] {
        write(closed);
        assert!(open_plan(dir.path(), Some(path)).is_none(), "{closed}");
    }
    for open in ["pending", "in_progress"] {
        write(open);
        let plan = open_plan(dir.path(), Some(path));
        assert_eq!(plan.map(|p| p.meta.todos.len()), Some(2), "{open}");
    }
}

#[test]
fn a_list_with_every_item_closed_is_dropped() {
    let item = |status| TodoItem {
        id: "a".into(),
        content: "A".into(),
        status,
        assignee: None,
    };
    let done = vec![item(TodoStatus::Completed), item(TodoStatus::Cancelled)];
    assert!(unfinished_todos(done).is_empty());
    for open in [TodoStatus::Pending, TodoStatus::InProgress] {
        let list = vec![item(TodoStatus::Completed), item(open)];
        assert_eq!(unfinished_todos(list.clone()), list, "{open:?}");
    }
    assert!(unfinished_todos(Vec::new()).is_empty());
}

#[test]
fn a_plan_is_finished_once_it_has_todos_and_all_are_closed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(PLANS_DIR)).unwrap();
    let path = ".aura/plans/p_00000001.plan.md";
    let write = |todos: &str| {
        std::fs::write(dir.path().join(path), format!("---\nname: P\n{todos}---\n")).unwrap();
    };

    assert!(!is_finished_plan(dir.path(), path), "no plan file");
    write("todos: []\n");
    assert!(!is_finished_plan(dir.path(), path), "no todos yet");
    write("todos:\n  - id: a\n    content: A\n    status: in_progress\n");
    assert!(!is_finished_plan(dir.path(), path), "a todo open");
    write("todos:\n  - id: a\n    content: A\n    status: completed\n");
    assert!(is_finished_plan(dir.path(), path));
    assert!(!is_finished_plan(dir.path(), "notes/p.plan.md"));
}

#[test]
fn a_list_with_the_plans_todo_ids_is_its_build_list() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(PLANS_DIR)).unwrap();
    write_plan(dir.path(), "p_00000001.plan.md", "P", 0);
    let path = ".aura/plans/p_00000001.plan.md";
    let item = |id: &str, status| TodoItem {
        id: id.into(),
        content: "anything".into(),
        status,
        assignee: None,
    };

    // Status and content may change while building; the ids decide.
    assert!(is_plan_todo_list(
        dir.path(),
        path,
        &[item("a", TodoStatus::InProgress)]
    ));
    assert!(!is_plan_todo_list(
        dir.path(),
        path,
        &[item("b", TodoStatus::Pending)]
    ));
    assert!(!is_plan_todo_list(dir.path(), path, &[]));
    assert!(!is_plan_todo_list(
        dir.path(),
        ".aura/plans/missing.plan.md",
        &[item("a", TodoStatus::Pending)]
    ));
}
