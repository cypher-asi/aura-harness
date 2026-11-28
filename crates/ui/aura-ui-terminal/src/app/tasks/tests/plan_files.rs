use super::*;

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
    assert_eq!(shape(app.task_list()), ["=Earlier", "-No Plan", "x", "@pa"]);

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
    assert_eq!(shape(app.task_list()), ["=Earlier", "-a", "#a 0", "#a 1"]);

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

    app.click_task_row(11, 6);
    assert!(app.detail_view().is_none());
    assert_eq!(shape(app.task_list()), ["=Earlier", "+a"]);
    app.click_task_row(20, 6);
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
