use super::*;

#[test]
fn test_processes_tab_lists_processes() {
    use crate::app::PanelFocus;
    use crate::events::{ProcessKind, ProcessStatus, ProcessSummary, UiCommand};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = crate::App::new();
    app.process_command(UiCommand::SetProcesses(vec![
        ProcessSummary {
            id: "turn-1".into(),
            kind: ProcessKind::Turn,
            label: "Agent".into(),
            status: ProcessStatus::Running,
            ..ProcessSummary::default()
        },
        ProcessSummary {
            id: "cmd-1".into(),
            kind: ProcessKind::Command,
            label: "cargo build".into(),
            status: ProcessStatus::Exited,
            depth: 1,
            ..ProcessSummary::default()
        },
    ]));
    while app.focus() != PanelFocus::Sandbox {
        let _ = app.handle_key(KeyEvent::from(KeyCode::Tab));
    }
    let _ = app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    let screen = render_rows(&mut app, 175).join("\n");
    let row = screen.lines().find(|r| r.contains("cargo build")).unwrap();
    assert!(row.contains("command"), "{row}");
    assert!(screen
        .lines()
        .any(|r| r.contains("Agent") && r.contains("turn")));
}

#[test]
fn test_trace_tab_lists_live_steps() {
    use crate::app::PanelFocus;
    use crate::events::{TraceNode, TraceNodeStatus, UiCommand};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = crate::App::new();
    app.process_command(UiCommand::GraphUpsert {
        run_id: "r".into(),
        nodes: vec![
            TraceNode {
                id: "s2".into(),
                kind: "iteration".into(),
                label: "model → edit_file".into(),
                status: TraceNodeStatus::Ok,
                seq: Some(2),
                ..TraceNode::default()
            },
            TraceNode {
                id: "d3".into(),
                kind: "decision".into(),
                label: "implement_now".into(),
                status: TraceNodeStatus::Warn,
                seq: Some(3),
                ..TraceNode::default()
            },
        ],
    });
    while app.focus() != PanelFocus::Sandbox {
        let _ = app.handle_key(KeyEvent::from(KeyCode::Tab));
    }
    // Sandbox, then back past TASKS to TRACE.
    for _ in 0..2 {
        let _ = app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    }
    let screen = render_rows(&mut app, 160).join("\n");
    assert!(!screen.contains("ISSUES"), "{screen}");
    let row = screen
        .lines()
        .find(|r| r.contains("implement_now"))
        .unwrap();
    assert!(row.contains("#3"), "{row}");
    assert!(screen.lines().any(|r| r.contains("model → edit_file")));
}

#[test]
fn test_files_command_toggles_sandbox_panel() {
    let mut app = crate::App::new();
    submit(&mut app, "/kernel");
    submit(&mut app, "/files");
    let screen = render_rows(&mut app, 150).join("\n");
    assert!(!screen.contains("KRN"));
    assert!(!screen.contains("TASKS"));

    submit(&mut app, "/files");
    let screen = render_rows(&mut app, 150).join("\n");
    assert!(
        screen.contains("TSK") || screen.contains("TASKS"),
        "{screen}"
    );
    assert!(!screen.contains("KRN"));
}

#[test]
fn test_tasks_tab_lists_plans_with_their_todos_above_their_tasks() {
    use crate::app::PanelFocus;
    use crate::events::{PlanView, TaskState, TaskView, TodoState, TodoView, UiCommand};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let todo = |content: &str, state| TodoView {
        content: content.into(),
        state,
        assignee: None,
        task: None,
    };
    let auth_path = ".aura/plans/auth_00000001.plan.md";
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetPlans(vec![
        PlanView {
            path: auth_path.into(),
            name: "Auth rework".into(),
            todos: vec![
                todo("Add session table", TodoState::Completed),
                TodoView {
                    task: Some("handler".into()),
                    ..todo("Write the login handler", TodoState::InProgress)
                },
            ],
        },
        PlanView {
            path: ".aura/plans/parser_00000002.plan.md".into(),
            name: "Parser cleanup".into(),
            todos: vec![todo("Split the lexer", TodoState::Completed)],
        },
    ]));
    app.process_command(UiCommand::SetTasks(vec![
        TaskView {
            id: "handler".into(),
            title: "Write the handler".into(),
            state: TaskState::Completed,
            assignee: Some("coder".into()),
            parent: Some("auth".into()),
            ..TaskView::default()
        },
        TaskView {
            id: "auth".into(),
            title: "Build the auth".into(),
            state: TaskState::Open,
            agent_name: "Lead".into(),
            plan: Some(auth_path.into()),
            builds: true,
            ..TaskView::default()
        },
    ]));
    while app.focus() != PanelFocus::Sandbox {
        let _ = app.handle_key(KeyEvent::from(KeyCode::Tab));
    }
    let _ = app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    let rows = render_rows(&mut app, 160);
    let row_with = |text: &str| {
        rows.iter()
            .position(|r| r.contains(text))
            .unwrap_or_else(|| panic!("no {text} in:\n{}", rows.join("\n")))
    };
    let auth = row_with("Auth rework");
    assert!(
        rows[auth].contains('▾') && rows[auth].contains("1/2"),
        "{}",
        rows[auth]
    );
    assert_eq!(row_with("Add session table"), auth + 1);
    assert!(rows[auth + 1].contains('☒'));
    assert!(
        rows[auth + 2].contains("▣  Write the login handler"),
        "{}",
        rows[auth + 2]
    );
    assert!(
        rows[auth + 3].contains("✓  Write the login handler  →"),
        "the task doing a todo sits under it, titled by the todo: {}",
        rows[auth + 3]
    );
    // The todos' boxes sit right under the plan's fold arrow; only the
    // task under a todo is indented, one level.
    let last_column = |row: usize, glyph: char| {
        rows[row]
            .chars()
            .enumerate()
            .filter_map(|(i, c)| (c == glyph).then_some(i))
            .last()
            .unwrap_or_else(|| panic!("no {glyph} in {:?}", rows[row]))
    };
    let arrow = last_column(auth, '▾');
    assert_eq!(last_column(auth + 1, '☒'), arrow);
    assert_eq!(last_column(auth + 2, '▣'), arrow);
    assert_eq!(last_column(auth + 3, '✓'), arrow + 2);
    let parser = row_with("Parser cleanup");
    assert_eq!(parser, auth + 4);
    assert_eq!(last_column(parser, '▸'), arrow, "plan headers line up");
    assert!(
        rows[parser].contains("✓ 1/1"),
        "a finished plan is marked done: {}",
        rows[parser]
    );
    assert!(
        !rows[auth].contains("✓ 1/2"),
        "an open plan is not: {}",
        rows[auth]
    );
    let screen = rows.join("\n");
    assert!(
        !screen.contains("Split the lexer"),
        "done plans start folded"
    );
    assert!(
        !screen.contains("Build the auth") && !screen.contains("No Plan"),
        "the build is the plan's header: {screen}"
    );

    // Plans: open ones white, settled ones gray. Todos look as in the
    // checklist that follows the conversation: done ones faint, the one in
    // progress bold white. Unfocused, so no row wears the selection colors.
    while app.focus() == PanelFocus::Sandbox {
        let _ = app.handle_key(KeyEvent::from(KeyCode::Tab));
    }
    let theme = test_theme();
    let buffer = render_buffer(&mut app, 160);
    let fg_of = |text: &str| {
        let row = row_of(&buffer, text);
        find_cell(&buffer, row, text).unwrap().1.fg
    };
    let muted = Some(theme.colors.muted);
    let foreground = Some(theme.colors.foreground);
    let faint = Some(theme.colors.faint);
    assert_eq!(fg_of("Add session table"), faint, "completed todo");
    assert_eq!(
        fg_of("Write the login handler"),
        foreground,
        "todo in progress"
    );
    let doing_row = row_of(&buffer, "Write the login handler");
    let (_, doing) = find_cell(&buffer, doing_row, "Write the login handler").unwrap();
    assert!(
        doing.add_modifier.contains(Modifier::BOLD),
        "todo in progress"
    );
    assert_eq!(fg_of("Parser cleanup"), muted, "plan with nothing left");
    assert_eq!(fg_of("Auth rework"), foreground, "plan with todos left");
    let check_row = row_of(&buffer, "Add session table");
    let (_, check) = find_cell(&buffer, check_row, "☒").unwrap();
    assert_eq!(check.fg, faint);
}

#[test]
fn test_active_plan_shows_in_the_side_panel_not_above_the_input() {
    use crate::events::{PlanView, TodoState, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::SetPlan(Some(PlanView {
        path: ".aura/plans/mnda_00000001.plan.md".into(),
        name: "Edit the MNDA requirement".into(),
        todos: vec![
            todo("Add setInviteMnda", TodoState::Pending),
            todo("Typecheck and lint", TodoState::Pending),
        ],
    })));
    let rows = render_rows(&mut app, 160);
    let screen = rows.join("\n");
    assert!(
        !rows.iter().any(|r| r.trim_start().starts_with("PLAN ")),
        "{screen}"
    );
    let plan_row = rows
        .iter()
        .find(|r| r.contains("Edit the MNDA requirement"))
        .unwrap_or_else(|| panic!("plan missing from:\n{screen}"));
    assert!(
        plan_row.find("Edit the MNDA").unwrap() > plan_row.len() / 2,
        "{plan_row}"
    );
    assert!(screen.contains("Add setInviteMnda"), "{screen}");
}
