
use super::*;
use crate::app::{DetailView, PanelFocus, SidePanelTab};
use crate::events::{TaskState, TodoState, TodoView, UiCommand, UiEvent};
use tokio::sync::mpsc;

use TaskState::{Completed as C, Failed as F, Running as R};
use TodoState::{Completed as Done, InProgress as Doing, Pending as Todo};

fn switched(rx: &mut mpsc::Receiver<UiEvent>) -> Option<String> {
    match rx.try_recv() {
        Ok(UiEvent::SelectAgent(id)) => Some(id),
        _ => None,
    }
}

fn notice(app: &App) -> Option<&str> {
    app.notification().map(|(text, _)| text.as_str())
}

fn item(content: &str, state: TodoState, task: Option<&str>) -> TodoView {
    TodoView {
        content: content.to_string(),
        state,
        assignee: None,
        task: task.map(str::to_string),
    }
}

/// Request `id` of agent `agent`, with `items`.
fn request(id: &str, agent: &str, items: Vec<TodoView>) -> TaskView {
    TaskView {
        id: id.to_string(),
        title: format!("asked {id}"),
        state: R,
        agent_id: agent.to_string(),
        agent_name: agent.to_uppercase(),
        items,
        ..TaskView::default()
    }
}

/// Task `id` handed out by `parent`, with an item of its own the tab
/// never lists.
fn part(id: &str, parent: &str, state: TaskState) -> TaskView {
    TaskView {
        id: id.to_string(),
        title: format!("do {id}"),
        state,
        agent_id: format!("agent-{id}"),
        agent_name: format!("Agent {id}"),
        assignee: Some("coder".to_string()),
        parent: Some(parent.to_string()),
        items: vec![item(&format!("inside {id}"), Todo, None)],
        ..TaskView::default()
    }
}

fn path(name: &str) -> String {
    format!(".aura/plans/{name}.plan.md")
}

fn plan(name: &str, states: &[TodoState]) -> PlanView {
    PlanView {
        path: path(name),
        name: name.to_string(),
        todos: states
            .iter()
            .enumerate()
            .map(|(i, &state)| item(&format!("{name} {i}"), state, None))
            .collect(),
    }
}

/// The rows as text, indented two spaces per level: `-`/`+` before
/// what is open/folded, `#` before a todo, `~` before a round's first
/// request, `@` before a task; items are their content.
fn shape(list: &TaskList) -> Vec<String> {
    list.rows()
        .iter()
        .map(|&row| {
            let fold = match row.fold() {
                Some(true) => "-",
                Some(false) => "+",
                None => "",
            };
            let text = match row {
                TaskRow::Plan { plan, .. } => plan
                    .map_or("No Plan", |plan| list.plans()[plan].name.as_str())
                    .to_string(),
                TaskRow::Todo { plan, todo, .. } => {
                    format!("#{}", list.plans()[plan].todos[todo].content)
                }
                TaskRow::Item { task, item, .. } => list.tasks()[task].items[item].content.clone(),
                TaskRow::Round { first, .. } => format!("~{}", list.tasks()[first].id),
                TaskRow::Task { task, .. } => format!("@{}", list.tasks()[task].id),
            };
            let text = match row {
                TaskRow::Todo { .. } => format!("#{fold}{}", &text[1..]),
                TaskRow::Round { .. } => format!("~{fold}{}", &text[1..]),
                _ => format!("{fold}{text}"),
            };
            format!("{}{text}", "  ".repeat(row.depth()))
        })
        .collect()
}

fn list_of(plans: Vec<PlanView>, tasks: Vec<TaskView>) -> TaskList {
    let mut list = TaskList::default();
    list.set_plans(plans);
    list.set_tasks(tasks);
    list
}

/// An app on the focused TASKS tab listing `plans` and `tasks`.
fn app_on_tasks(plans: Vec<PlanView>, tasks: Vec<TaskView>) -> App {
    let mut app = App::new();
    app.process_command(UiCommand::SetPlans(plans));
    app.process_command(UiCommand::SetTasks(tasks));
    app.focus = PanelFocus::Sandbox;
    app.show_side_tab(SidePanelTab::Tasks);
    app
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::from(code));
}

fn selected_shape(app: &App) -> String {
    shape(app.task_list())[app.task_list().selected()].clone()
}

#[test]
fn requests_have_no_row_their_items_and_parts_do_at_the_left_edge() {
    let list = list_of(
        Vec::new(),
        vec![
            part("p", "r", R),
            request(
                "r",
                "lead",
                vec![item("x", Done, None), item("y", Todo, None)],
            ),
        ],
    );
    assert_eq!(shape(&list), ["-No Plan", "x", "y", "@p"]);
    assert!(
        list.rows().iter().all(|row| row.depth() == 0),
        "icons sit under the header's arrow"
    );
}

#[test]
fn a_request_with_nothing_to_list_adds_no_rows() {
    let list = list_of(Vec::new(), vec![request("r", "lead", Vec::new())]);
    assert!(list.rows().is_empty(), "{:?}", shape(&list));
}

#[test]
fn an_agents_list_is_merged_across_its_requests_newest_state_wins() {
    let list = list_of(
        Vec::new(),
        vec![
            request(
                "commit",
                "lead",
                vec![
                    item("a", Done, None),
                    item("b", Done, None),
                    item("c", Todo, None),
                ],
            ),
            request(
                "impl",
                "lead",
                vec![item("a", Done, None), item("b", Doing, None)],
            ),
        ],
    );
    assert_eq!(shape(&list), ["-No Plan", "a", "b", "c"]);
    let TaskRow::Item { task, item, .. } = list.rows()[2] else {
        panic!("an item row");
    };
    assert_eq!(list.item(task, item).map(|i| i.state), Some(Done));
}

#[test]
fn the_agent_last_at_work_comes_first() {
    let list = list_of(
        Vec::new(),
        vec![
            request("new", "b", vec![item("b's", Todo, None)]),
            request("old", "a", vec![item("a's", Todo, None)]),
        ],
    );
    assert_eq!(shape(&list), ["-No Plan", "b's", "a's"]);
}

#[test]
fn a_task_sits_one_level_under_the_item_it_does_even_after_the_list_moves_on() {
    let tasks = vec![
        request(
            "later",
            "lead",
            vec![item("x", Todo, None), item("y", Todo, None)],
        ),
        part("p2", "first", R),
        part("p1", "first", R),
        request(
            "first",
            "lead",
            vec![item("x", Todo, Some("p1")), item("y", Todo, None)],
        ),
    ];
    let list = list_of(Vec::new(), tasks);
    assert_eq!(
        shape(&list),
        ["-No Plan", "-x", "  @p1", "y", "@p2"],
        "p1 does x, which the later list carries without the link"
    );
}

#[test]
fn a_tasks_own_parts_nest_under_it_and_its_items_are_not_listed() {
    let list = list_of(
        Vec::new(),
        vec![
            part("q", "p", R),
            part("p", "r", R),
            request("r", "lead", Vec::new()),
        ],
    );
    assert_eq!(shape(&list), ["-No Plan", "-@p", "  @q"]);
}

#[test]
fn done_items_fold_their_tasks_and_folds_stick() {
    let tasks = || {
        vec![
            part("p1", "r", C),
            request("r", "lead", vec![item("x", Done, Some("p1"))]),
        ]
    };
    let mut app = app_on_tasks(Vec::new(), tasks());
    assert_eq!(shape(app.task_list()), ["-No Plan", "+x"]);

    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Right);
    assert_eq!(shape(app.task_list()), ["-No Plan", "-x", "  @p1"]);
    app.process_command(UiCommand::SetTasks(tasks()));
    assert_eq!(
        shape(app.task_list()),
        ["-No Plan", "-x", "  @p1"],
        "the fold survives updates"
    );
    assert_eq!(selected_shape(&app), "-x", "and so does the highlight");

    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Left);
    assert_eq!(
        shape(app.task_list()),
        ["-No Plan", "+x"],
        "Left on a task with nothing under it folds the item it is under"
    );
    assert_eq!(selected_shape(&app), "+x");
}

#[test]
fn a_new_request_keeps_the_highlight_on_its_item() {
    let mut app = app_on_tasks(
        Vec::new(),
        vec![request(
            "one",
            "lead",
            vec![item("x", Todo, None), item("y", Todo, None)],
        )],
    );
    press(&mut app, KeyCode::End);
    assert_eq!(selected_shape(&app), "y");
    app.process_command(UiCommand::UpsertTask(request(
        "two",
        "other",
        vec![item("z", Todo, None)],
    )));
    assert_eq!(shape(app.task_list()), ["-No Plan", "z", "x", "y"]);
    assert_eq!(selected_shape(&app), "y");
}

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

    // Column 1 of the rows holds every icon at the left edge.
    app.click_task_row(11, 6);
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

/// An app on the TASKS tab of a temp sandbox holding `.aura/plans/a.plan.md`.
fn app_with_plan_file(text: &str) -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aura/plans")).unwrap();
    std::fs::write(dir.path().join(".aura/plans/a.plan.md"), text).unwrap();
    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    app.focus = PanelFocus::Sandbox;
    app.show_side_tab(SidePanelTab::Tasks);
    (app, dir)
}

/// The todo the open plan document is focused on.
fn open_plan_focus(app: &App) -> Option<usize> {
    let Some(DetailView::Plan(doc)) = app.detail_view() else {
        panic!("the plan opens as a document");
    };
    doc.focus
}

#[test]
fn p_opens_a_tasks_plan_at_its_todo_and_says_when_there_is_none() {
    let (mut app, _dir) = app_with_plan_file(
        "---\nname: a\ntodos:\n- id: one\n  content: first\n- id: two\n  content: second\n---\n",
    );
    let mut handed = part("pa", "r", R);
    handed.plan = Some(path("a"));
    handed.plan_todo = Some("two".to_string());
    app.process_command(UiCommand::SetTasks(vec![
        handed,
        request("r", "lead", vec![item("x", Todo, None)]),
    ]));
    assert_eq!(shape(app.task_list()), ["-No Plan", "x", "@pa"]);

    press(&mut app, KeyCode::Char('p'));
    assert_eq!(notice(&app), Some("No plan to open"));
    press(&mut app, KeyCode::End);
    press(&mut app, KeyCode::Char('p'));
    assert_eq!(open_plan_focus(&app), Some(1), "the plan opens at the todo");
}

#[test]
fn enter_on_a_plan_header_opens_its_file_and_on_a_todo_at_the_todo() {
    let (mut app, _dir) =
        app_with_plan_file("---\nname: a\ntodos:\n- content: a 0\n- content: a 1\n---\n");
    app.process_command(UiCommand::SetPlans(vec![plan("a", &[Todo, Todo])]));
    assert_eq!(shape(app.task_list()), ["-a", "#a 0", "#a 1"]);

    press(&mut app, KeyCode::Enter);
    assert_eq!(
        open_plan_focus(&app),
        None,
        "the header opens the plan at the top"
    );
    app.close_detail();
    press(&mut app, KeyCode::End);
    press(&mut app, KeyCode::Enter);
    assert_eq!(open_plan_focus(&app), Some(1), "a todo opens at the todo");
    app.close_detail();
    press(&mut app, KeyCode::Char('p'));
    assert_eq!(
        open_plan_focus(&app),
        Some(1),
        "p on a todo opens the plan too"
    );
}

#[test]
fn a_click_opens_a_plan_and_its_fold_glyph_folds_it() {
    let (mut app, _dir) = app_with_plan_file("a\n");
    app.process_command(UiCommand::SetPlans(vec![plan("a", &[Todo])]));
    app.set_task_rows_area(Rect::new(10, 5, 30, 8), 0);

    app.click_task_row(11, 5);
    assert!(app.detail_view().is_none());
    assert_eq!(shape(app.task_list()), ["+a"]);
    app.click_task_row(20, 5);
    assert!(matches!(app.detail_view(), Some(DetailView::Plan(_))));
}

#[test]
fn a_new_plan_opens_the_tasks_tab_but_its_updates_do_not() {
    let mut app = App::new();
    app.process_command(UiCommand::SetPlan(Some(plan("a", &[Todo, Todo]))));
    assert_eq!(app.side_tab(), SidePanelTab::Tasks);

    app.show_side_tab(SidePanelTab::Sandbox);
    app.process_command(UiCommand::SetPlan(Some(plan("a", &[Done, Todo]))));
    assert_eq!(app.side_tab(), SidePanelTab::Sandbox);

    app.process_command(UiCommand::SetPlan(Some(plan("b", &[Todo]))));
    assert_eq!(app.side_tab(), SidePanelTab::Tasks);
}

#[test]
fn set_plan_marks_the_listed_plan() {
    let mut app = App::new();
    app.process_command(UiCommand::SetPlans(vec![
        plan("a", &[Todo]),
        plan("b", &[Done]),
    ]));
    app.process_command(UiCommand::SetPlan(Some(plan("b", &[Done, Todo]))));
    let list = app.task_list();
    assert!(list.is_current(&list.plans()[1]));
    assert_eq!(list.plans()[1].todos.len(), 2);
}

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
    assert_eq!(shape(&list), ["-ch", "#ch 0", "#ch 1", "~+b1", "@r1"]);
    let TaskRow::Round { progress, .. } = list.rows()[3] else {
        panic!("a round row");
    };
    assert_eq!(progress, (3, 3), "the items merged across b1 and b2");

    list.select(3);
    list.toggle_selected();
    assert_eq!(
        shape(&list),
        ["-ch", "#ch 0", "#ch 1", "~-b1", "+old a", "+old b", "old c", "@r1"]
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
            "-ch", "#ch 0", "#ch 1", "~-b1", "-old a", "  @t1", "  @t2", "  @t3", "+old b",
            "old c", "@r1"
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
    assert_eq!(shape(&list), ["-ch", "#ch 0", "~+b2", "~+b1"]);
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
        ["-ch", "#ch 0", "#ch 1", "~+b1", "@r1"]
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
    assert_eq!(shape(&list), ["-ch", "#-ch 0", "  @n1", "  @n2"]);
    assert_eq!(stale_of(&list, "n1"), Some(true));
    assert_eq!(stale_of(&list, "n2"), Some(false));
}
