use super::*;

#[test]
fn test_a_dragged_project_shows_its_hit_area() {
    use crate::events::{AgentSummary, UiCommand};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let theme = test_theme();
    let agent = |id: &str, project: &str| AgentSummary {
        id: id.to_string(),
        name: format!("Agent {id}"),
        project_id: Some(project.to_lowercase()),
        project: Some(project.to_string()),
        ..Default::default()
    };
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![
        agent("a", "Alpha"),
        agent("b", "Beta"),
    ]));
    let buffer = render_buffer(&mut app, 120);
    let alpha = row_of(&buffer, "ALPHA");
    let beta = row_of(&buffer, "BETA");
    let (column, _) = find_cell(&buffer, alpha, "ALPHA").unwrap();
    let press = |app: &mut crate::App, kind, row| {
        app.handle_mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        });
    };
    press(&mut app, MouseEventKind::Down(MouseButton::Left), alpha);
    // A rule cell in the list's columns, clear of the other panels.
    let rule_at = |buffer: &ratatui::buffer::Buffer, y: u16| {
        let cell = &buffer[(column, y)];
        (cell.symbol() == "─").then(|| cell.style())
    };
    let buffer = render_buffer(&mut app, 120);
    assert!(rule_at(&buffer, beta - 1).is_none(), "no line on a press");

    press(&mut app, MouseEventKind::Drag(MouseButton::Left), beta);
    let buffer = render_buffer(&mut app, 120);
    // Alpha took Beta's place under the pointer: lifted, with a line in
    // the gap between Beta and itself, where it went.
    let moved = row_of(&buffer, "ALPHA");
    assert_eq!(moved, beta);
    let line = rule_at(&buffer, moved - 1).expect("line above the dragged rows");
    assert_eq!(line.fg, Some(theme.colors.success));
    let (_, name) = find_cell(&buffer, moved, "ALPHA").unwrap();
    assert_eq!(name.bg, Some(theme.colors.surface_hover));
    assert!(
        find_cell(&buffer, moved, "▎").is_none(),
        "no rail beside the rows"
    );
    assert!(rule_at(&buffer, moved + 2).is_none(), "one line only");

    press(&mut app, MouseEventKind::Up(MouseButton::Left), beta);
    let buffer = render_buffer(&mut app, 120);
    assert!(rule_at(&buffer, beta - 1).is_none(), "gone on drop");
}

#[test]
fn test_a_dragged_agent_shows_a_line_between_agents() {
    use crate::events::{AgentSummary, UiCommand};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::style::Modifier;

    let theme = test_theme();
    let agent = |id: &str| AgentSummary {
        id: id.to_string(),
        name: format!("Agent {id}"),
        project_id: Some("alpha".to_string()),
        project: Some("Alpha".to_string()),
        ..Default::default()
    };
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![
        agent("a"),
        agent("b"),
        agent("c"),
    ]));
    let buffer = render_buffer(&mut app, 120);
    let first = row_of(&buffer, "Agent a");
    let third = row_of(&buffer, "Agent c");
    let (column, _) = find_cell(&buffer, first, "Agent").unwrap();
    let press = |app: &mut crate::App, kind, row| {
        app.handle_mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        });
    };
    press(&mut app, MouseEventKind::Down(MouseButton::Left), first);
    press(&mut app, MouseEventKind::Drag(MouseButton::Left), first + 1);
    press(&mut app, MouseEventKind::Drag(MouseButton::Left), third);
    let buffer = render_buffer(&mut app, 120);
    // a went down past b and then c: the line runs under c, above a.
    assert_eq!(row_of(&buffer, "Agent a"), third);
    let (_, above) = find_cell(&buffer, third - 1, "Agent").unwrap();
    assert!(above.add_modifier.contains(Modifier::UNDERLINED));
    assert_eq!(above.underline_color, Some(theme.colors.success));
    let (_, dragged) = find_cell(&buffer, third, "Agent").unwrap();
    assert!(!dragged.add_modifier.contains(Modifier::UNDERLINED));
}

#[test]
fn test_todos_stay_with_their_agent() {
    use crate::events::{AgentSummary, TodoState, UiCommand};

    let agent = |id: &str| AgentSummary {
        id: id.to_string(),
        name: id.to_string(),
        ..Default::default()
    };
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![agent("aa"), agent("bb")]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(UiCommand::ForAgent {
        agent_id: "bb".to_string(),
        command: Box::new(UiCommand::SetTodos(vec![todo(
            "Background work",
            TodoState::InProgress,
        )])),
    });

    let screen = render_rows(&mut app, 160).join("\n");
    assert!(!screen.contains("Background work"), "{screen}");

    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    // The drawer shows the agent's list by itself, no `/todos` needed.
    let screen = render_rows(&mut app, 160).join("\n");
    assert!(screen.contains("◐ Background work"), "{screen}");
}

#[test]
fn test_status_line_leads_with_the_mode() {
    use crate::events::{CallStats, UiCommand};

    let mut app = crate::App::new();
    let rows = render_rows(&mut app, 160);
    assert!(!rows[rows.len() - 1].contains("mode"), "unknown until told");

    app.process_command(UiCommand::SetMode(crate::PermissionMode::ReadOnly));
    app.process_command(UiCommand::CallStats(CallStats {
        input_tokens: 10_000,
        context_limit: Some(200_000),
        ..CallStats::default()
    }));
    let rows = render_rows(&mut app, 160);
    let status = &rows[rows.len() - 1];
    let mode = status.find("read-only mode").expect(status);
    let ctx = status.find("5% context used").expect(status);
    assert!(mode < ctx, "{status:?}");
}

#[test]
fn test_approval_prompt_offers_three_answers() {
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    app.process_command(UiCommand::RequestApproval {
        id: "req-1".to_string(),
        tool: "run_command".to_string(),
        description: "cargo test -p aura-terminal".to_string(),
        always: "`cargo test` commands in this project".to_string(),
    });
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(screen.contains("APPROVAL REQUIRED"), "{screen}");
    assert!(screen.contains("run_command wants to:"), "{screen}");
    assert!(screen.contains("cargo test -p aura-terminal"), "{screen}");
    assert!(screen.contains("[Y] Allow once"), "{screen}");
    assert!(
        screen.contains("[A] Always allow `cargo test` commands in this project"),
        "{screen}"
    );
    assert!(screen.contains("[N] Deny"), "{screen}");
    assert!(!screen.contains("Esc"), "no key hints: {screen}");
}

#[test]
fn test_help_lists_mode_command_and_shift_tab() {
    let mut app = crate::App::new();
    run_slash(&mut app, "/help");
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 50)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, &mut app, &test_theme()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let screen: String = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                + "\n"
        })
        .collect();
    assert!(screen.contains(" HELP "), "{screen}");
    assert!(screen.contains("/mode [name]"), "{screen}");
    assert!(
        screen.contains(&format!("{:<24}{}", "/model [opus]", "Pick the model")),
        "fits the help box on one line: {screen}"
    );
    assert!(screen.contains("Shift+Tab"), "{screen}");
    assert!(!screen.contains("Press any key"), "no key hints: {screen}");
}

#[test]
fn test_init_command_asks_the_active_agent_and_shows_it_in_the_menu() {
    use crate::events::UiEvent;

    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let mut app = crate::App::new().with_event_sender(tx);
    app.process_command(crate::events::UiCommand::SetActiveAgent("aa".to_string()));

    type_text(&mut app, "/ini");
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(
        screen.contains("/init") && screen.contains("write or improve AGENTS.md"),
        "{screen}"
    );

    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::empty(),
    ));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::InitProject { agent_id }) if agent_id == "aa"
    ));
    assert_eq!(app.state(), crate::app::AppState::Processing);
}

#[test]
fn test_context_command_lists_project_instructions() {
    use crate::events::{InstructionFileView, InstructionStatus, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::SetInstructions(vec![InstructionFileView {
        path: "~/repo/AGENTS.md".to_string(),
        bytes: 2_000,
        globs: None,
        status: InstructionStatus::Included,
    }]));
    run_slash(&mut app, "/context text");
    run_slash(&mut app, "/new");
    run_slash(&mut app, "/context text");

    let texts: Vec<String> = app
        .messages()
        .iter()
        .map(|m| m.content().to_string())
        .collect();
    let last = texts.last().map(String::as_str).unwrap_or_default();
    assert!(
        last.contains("Project instructions (~500 tokens):"),
        "{texts:?}"
    );
    assert!(last.contains("~/repo/AGENTS.md  500"), "{texts:?}");
}

#[test]
fn test_spawn_guides_join_each_agent_to_its_parent() {
    use super::super::agents::projects::spawn_guides;
    use crate::events::AgentSummary;

    let agents: Vec<AgentSummary> = [0, 1, 2, 1, 2, 0, 1]
        .into_iter()
        .map(|depth| AgentSummary {
            depth,
            ..AgentSummary::default()
        })
        .collect();
    let guides: Vec<String> = (0..agents.len())
        .map(|index| spawn_guides(&agents, index))
        .collect();
    // Each level's line drops from under its parent's icon.
    let lead = " ".repeat(crate::animation::AURA_WIDTH + 1);
    let at = |marks: &[&str]| {
        marks
            .iter()
            .map(|mark| format!("{lead}{mark}"))
            .collect::<String>()
    };
    assert_eq!(
        guides,
        [
            String::new(),
            at(&["├"]),
            at(&["│", "└"]),
            at(&["└"]),
            at(&[" ", "└"]),
            String::new(),
            at(&["└"]),
        ]
    );
}

/// The Tasks tab, drawn for a request with two items and one part doing
/// none of them.
fn tasks_tab_rows() -> Vec<String> {
    use crate::app::PanelFocus;
    use crate::events::{TaskState, TaskView, TodoState, TodoView, UiCommand};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let todo = |content: &str, state, assignee: Option<&str>| TodoView {
        content: content.into(),
        state,
        assignee: assignee.map(str::to_string),
        task: None,
    };
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetTasks(vec![
        TaskView {
            id: "part".into(),
            title: "Parse the source".into(),
            state: TaskState::Completed,
            agent_name: "coder #1".into(),
            assignee: Some("coder".into()),
            parent: Some("request".into()),
            ..TaskView::default()
        },
        TaskView {
            id: "request".into(),
            title: "Build the compiler".into(),
            state: TaskState::Open,
            agent_name: "Lead".into(),
            items: vec![
                todo("Write the parser", TodoState::Completed, Some("coder")),
                todo("Write the checker", TodoState::Pending, None),
            ],
            ..TaskView::default()
        },
    ]));
    while app.focus() != PanelFocus::Sandbox {
        let _ = app.handle_key(KeyEvent::from(KeyCode::Tab));
    }
    // Sandbox, then back past HOST to TASKS.
    for _ in 0..2 {
        let _ = app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    }
    render_rows(&mut app, 200)
}

/// The column of the last `glyph` in `rows[row]`.
fn last_column(rows: &[String], row: usize, glyph: char) -> usize {
    rows[row]
        .chars()
        .enumerate()
        .filter_map(|(i, c)| (c == glyph).then_some(i))
        .last()
        .unwrap_or_else(|| panic!("no {glyph} in {:?}", rows[row]))
}

#[test]
fn test_tasks_tab_lists_requests_items_not_what_was_asked() {
    let rows = tasks_tab_rows();
    let row_with = |text: &str| {
        rows.iter()
            .position(|r| r.contains(text))
            .unwrap_or_else(|| panic!("no {text} in:\n{}", rows.join("\n")))
    };
    let screen = rows.join("\n");
    assert!(
        !screen.contains("Build the compiler") && !screen.contains("Lead"),
        "what the user asked has no row: {screen}"
    );
    let no_plan = row_with("No Plan");
    assert!(rows[no_plan].contains('▾'), "{}", rows[no_plan]);
    assert!(
        rows[no_plan + 1].contains("✓  Write the parser  → coder"),
        "{}",
        rows[no_plan + 1]
    );
    assert!(
        rows[no_plan + 2].contains("☐  Write the checker"),
        "{}",
        rows[no_plan + 2]
    );
    let part = row_with("Parse the source  → coder");
    assert_eq!(part, no_plan + 3, "the part doing no item follows them");
    // Every icon sits right under the header's fold arrow.
    let arrow = last_column(&rows, no_plan, '▾');
    assert_eq!(last_column(&rows, no_plan + 1, '✓'), arrow);
    assert_eq!(last_column(&rows, no_plan + 2, '☐'), arrow);
    assert_eq!(last_column(&rows, part, '✓'), arrow);
    assert!(!rows.iter().any(|r| r.contains("p plan")), "no keys");
}
