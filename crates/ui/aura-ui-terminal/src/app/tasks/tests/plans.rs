use super::*;

/// Plan `auth` with todos `auth 0`, which `p1` does, and `auth 1`;
/// `build` builds it with items of its own and hands out `p1` and
/// then `p2`.
fn building() -> (PlanView, Vec<TaskView>) {
    let mut auth = plan("auth", &[Doing, Todo]);
    auth.todos[0].task = Some("p1".to_string());
    let build = TaskView {
        plan: Some(path("auth")),
        builds: true,
        ..request("build", "lead", vec![item("own 0", Todo, None)])
    };
    let tasks = vec![part("p2", "build", R), part("p1", "build", R), build];
    (auth, tasks)
}

#[test]
fn the_build_adds_no_items_and_its_tasks_go_under_the_todos_they_do() {
    let (auth, tasks) = building();
    let list = list_of(vec![auth], tasks);
    assert_eq!(
        shape(&list),
        ["-auth", "#-auth 0", "  @p1", "#auth 1", "@p2"]
    );
    assert!(list.is_hidden_build("build"));
    assert!(!list.is_hidden_build("p1"), "only the request is hidden");
}

#[test]
fn a_build_whose_plan_is_not_listed_lists_its_items_under_no_plan() {
    let (_, tasks) = building();
    let list = list_of(vec![plan("other", &[Todo])], tasks);
    assert_eq!(
        shape(&list),
        ["-other", "#other 0", "-No Plan", "own 0", "@p1", "@p2"]
    );
    assert!(!list.is_hidden_build("build"));
}

#[test]
fn plans_with_open_work_start_open_and_done_ones_folded() {
    let list = list_of(
        vec![
            plan("todos", &[Todo]),
            plan("task", &[Done]),
            plan("done", &[Done]),
        ],
        vec![TaskView {
            plan: Some(path("task")),
            ..request("r", "lead", Vec::new())
        }],
    );
    assert_eq!(
        shape(&list),
        ["-todos", "#todos 0", "-task", "#task 0", "+done"]
    );
}

#[test]
fn the_current_plan_comes_first_and_starts_open_even_when_done() {
    let mut list = list_of(vec![plan("a", &[Todo]), plan("b", &[Done])], Vec::new());
    list.set_current_plan(Some(&plan("b", &[Done])));
    assert_eq!(shape(&list), ["-b", "#b 0", "-a", "#a 0"]);
    list.set_current_plan(Some(&plan("c", &[Todo])));
    assert_eq!(list.plans()[0].path, path("c"), "a new plan goes on top");
}

#[test]
fn selecting_a_folded_task_opens_its_group_and_item() {
    let mut list = list_of(
        Vec::new(),
        vec![
            part("p1", "r", C),
            request("r", "lead", vec![item("x", Done, Some("p1"))]),
        ],
    );
    list.select(0);
    list.toggle_selected();
    assert_eq!(shape(&list), ["+No Plan"]);
    assert!(list.select_task("p1"));
    assert_eq!(shape(&list), ["-No Plan", "-x", "  @p1"]);
    assert_eq!(list.selected(), 2);
    assert!(!list.select_task("missing"));
    assert!(
        !list.select_task("r"),
        "a request has no row; its group's header is highlighted"
    );
    assert_eq!(list.selected(), 0);
}

#[test]
fn t_selects_the_task_doing_a_todo_or_item_or_says_why_not() {
    let (mut auth, mut tasks) = building();
    auth.todos[1].task = Some("build".to_string());
    tasks.push(part("p3", "r", R));
    tasks.push(request(
        "r",
        "other",
        vec![item("x", Todo, Some("p3")), item("y", Todo, None)],
    ));
    let mut app = app_on_tasks(vec![auth], tasks);
    assert_eq!(
        shape(app.task_list()),
        ["-auth", "#-auth 0", "  @p1", "#auth 1", "@p2", "-No Plan", "-x", "  @p3", "y"]
    );

    press(&mut app, KeyCode::Char('t'));
    assert_eq!(notice(&app), Some("Select a todo to jump to its task"));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Char('t'));
    assert_eq!(selected_shape(&app), "  @p1", "the todo opens to show it");

    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Char('t'));
    assert_eq!(notice(&app), Some("This todo is part of the plan's build"));
    assert_eq!(selected_shape(&app), "#auth 1", "the highlight stays put");

    press(&mut app, KeyCode::End);
    press(&mut app, KeyCode::Char('t'));
    assert_eq!(notice(&app), Some("No task is doing this todo"));
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Char('t'));
    assert_eq!(selected_shape(&app), "  @p3", "an item's task");
}

#[test]
fn enter_and_clicks_show_the_agent_and_fold_on_the_icon() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetTasks(vec![
        part("p", "r", R),
        request("r", "lead", vec![item("x", Todo, Some("p"))]),
    ]));
    app.show_side_tab(SidePanelTab::Tasks);
    app.set_task_rows_area(Rect::new(10, 5, 30, 8), 0);
    assert_eq!(shape(app.task_list()), ["-No Plan", "-x", "  @p"]);

    // Column 2 of the rows, past their inset, holds every icon at the
    // left edge.
    app.click_task_row(12, 6);
    assert_eq!(shape(app.task_list()), ["-No Plan", "+x"]);
    app.click_task_row(20, 6);
    assert_eq!(
        switched(&mut rx).as_deref(),
        Some("lead"),
        "an item shows the agent of its request"
    );
    app.click_task_row(20, 5);
    assert_eq!(
        shape(app.task_list()),
        ["+No Plan"],
        "No Plan folds anywhere"
    );
}
