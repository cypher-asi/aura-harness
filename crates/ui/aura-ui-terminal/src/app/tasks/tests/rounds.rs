use super::*;

/// Request `id` of `lead` building plan `ch`, with `items`.
fn build(id: &str, items: Vec<TodoView>) -> TaskView {
    TaskView {
        plan: Some(path("ch")),
        builds: true,
        ..request(id, "lead", items)
    }
}

/// Task `id` handed out by `parent` for todo `todo` of plan `ch`.
fn did(id: &str, parent: &str, state: TaskState, todo: &str) -> TaskView {
    TaskView {
        plan: Some(path("ch")),
        plan_todo: Some(todo.to_string()),
        ..part(id, parent, state)
    }
}

/// Plan `ch`, whose list `b3` builds now. `b1` and `b2` built an
/// earlier list: `t1` and then `t2` failed at `old a`, `t3` did it,
/// `t4` did `old b`, nothing did `old c`, and `r1` did no item.
fn rewritten() -> (PlanView, Vec<TaskView>) {
    let tasks = vec![
        build(
            "b3",
            vec![item("ch 0", Done, None), item("ch 1", Doing, None)],
        ),
        part("r1", "b2", C),
        did("t4", "b2", C, "b"),
        did("t3", "b2", C, "a"),
        build(
            "b2",
            vec![
                item("old a", Done, Some("t3")),
                item("old b", Done, Some("t4")),
                item("old c", Done, None),
            ],
        ),
        did("t2", "b1", F, "a"),
        did("t1", "b1", F, "a"),
        build(
            "b1",
            vec![item("old a", Todo, Some("t2")), item("old b", Todo, None)],
        ),
    ];
    (plan("ch", &[Done, Doing]), tasks)
}

/// Whether the row of task `id` is stale; `None` when it has none.
fn stale_of(list: &TaskList, id: &str) -> Option<bool> {
    list.rows().iter().find_map(|&row| match row {
        TaskRow::Task { task, stale, .. } if list.tasks()[task].id == id => Some(stale),
        _ => None,
    })
}

#[test]
fn builds_of_a_list_the_plan_dropped_make_a_folded_round_at_the_left_edge() {
    let (ch, tasks) = rewritten();
    let mut list = list_of(vec![ch], tasks);
    assert_eq!(
        shape(&list),
        ["=Earlier", "-ch", "#ch 0", "#ch 1", "~+b1", "@r1"]
    );
    let TaskRow::Round { progress, .. } = list.rows()[4] else {
        panic!("a round row");
    };
    assert_eq!(progress, (3, 3), "the items merged across b1 and b2");

    list.select(4);
    list.toggle_selected();
    assert_eq!(
        shape(&list),
        ["=Earlier", "-ch", "#ch 0", "#ch 1", "~-b1", "+old a", "+old b", "old c", "@r1"]
    );
    assert!(
        list.rows().iter().all(|row| row.depth() == 0),
        "rounds and their items add no indentation"
    );
}

#[test]
fn every_try_at_a_rounds_item_nests_under_it_and_only_the_newest_is_not_stale() {
    let (ch, tasks) = rewritten();
    let mut list = list_of(vec![ch], tasks);
    assert!(list.select_task("t1"), "selecting opens the round and item");
    assert_eq!(
        shape(&list),
        [
            "=Earlier", "-ch", "#ch 0", "#ch 1", "~-b1", "-old a", "  @t1", "  @t2", "  @t3",
            "+old b", "old c", "@r1"
        ]
    );
    assert_eq!(shape(&list)[list.selected()], "  @t1");
    assert_eq!(stale_of(&list, "t1"), Some(true));
    assert_eq!(stale_of(&list, "t2"), Some(true));
    assert_eq!(stale_of(&list, "t3"), Some(false));
    assert_eq!(stale_of(&list, "r1"), Some(false));

    let title = |id: &str| {
        list.rows()
            .iter()
            .find(|&&row| matches!(row, TaskRow::Task { task, .. } if list.tasks()[task].id == id))
            .map(|&row| list.title_of(row))
    };
    assert_eq!(title("t1"), Some("old a"), "titled by the item it does");
    assert_eq!(title("r1"), Some("do r1"), "else by what it was asked");
}

#[test]
fn lists_sharing_no_item_make_separate_rounds_newest_first() {
    let tasks = vec![
        build("b3", vec![item("ch 0", Todo, None)]),
        build("b2", vec![item("y", Done, None)]),
        build("b1", vec![item("x", Done, None)]),
    ];
    let list = list_of(vec![plan("ch", &[Todo])], tasks);
    assert_eq!(shape(&list), ["=Earlier", "-ch", "#ch 0", "~+b2", "~+b1"]);
}

#[test]
fn round_folds_stick_and_left_on_its_item_folds_it() {
    let (ch, tasks) = rewritten();
    let mut app = app_on_tasks(vec![ch], tasks);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Right);
    assert_eq!(selected_shape(&app), "~-b1");
    app.process_command(UiCommand::SetTasks(rewritten().1));
    assert_eq!(selected_shape(&app), "~-b1", "the fold survives updates");

    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    assert_eq!(selected_shape(&app), "old c");
    press(&mut app, KeyCode::Left);
    assert_eq!(selected_shape(&app), "~+b1");
    assert_eq!(
        shape(app.task_list()),
        ["=Earlier", "-ch", "#ch 0", "#ch 1", "~+b1", "@r1"]
    );
}

#[test]
fn tasks_doing_the_same_todo_as_a_plans_todo_nest_under_it() {
    let mut p = plan("ch", &[Doing]);
    p.todos[0].task = Some("n2".to_string());
    let list = list_of(
        vec![p],
        vec![
            did("n2", "b", C, "z"),
            did("n1", "b", F, "z"),
            build("b", Vec::new()),
        ],
    );
    assert_eq!(
        shape(&list),
        ["=Earlier", "-ch", "#-ch 0", "  @n1", "  @n2"]
    );
    assert_eq!(stale_of(&list, "n1"), Some(true));
    assert_eq!(stale_of(&list, "n2"), Some(false));
}
