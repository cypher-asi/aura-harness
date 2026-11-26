use super::*;

#[test]
fn test_sandbox_panel_sits_below_record() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();

    let mut app = crate::App::new();
    app.process_command(crate::events::UiCommand::SetSandboxRoot(
        dir.path().to_path_buf(),
    ));
    let rows = render_rows(&mut app, 150);

    let (record, _) = header_bar(&rows, "KRN");
    let (sandbox, _) = header_bar(&rows, "SBX");
    assert!(sandbox > record + 10, "record={record} sandbox={sandbox}");
    let column = |row: &str, title: &str| row[..row.find(title).unwrap()].chars().count();
    // The Kernel panel's left frame: the last border before its first tab.
    let kernel_border = rows[record][..rows[record].find("KRN").unwrap()]
        .chars()
        .collect::<Vec<char>>()
        .iter()
        .rposition(|&c| c == '│')
        .unwrap();
    assert_eq!(
        rows[sandbox].chars().nth(kernel_border),
        Some('│'),
        "{}",
        rows[sandbox]
    );
    assert!(
        rows[sandbox].contains("TSK") || rows[sandbox].contains("TASKS"),
        "{}",
        rows[sandbox]
    );
    // The tabs, their bottom border, the body's top border and its blank
    // row come first.
    assert!(
        rows[sandbox + 4].contains("▸  src"),
        "{}",
        rows[sandbox + 4]
    );
    assert!(rows[sandbox + 5].contains("Cargo.toml"));
    assert_eq!(
        column(&rows[sandbox + 4], "▸  src"),
        kernel_border + 1 + crate::components::ROW_INSET,
        "tree rows share the Record panel's gutter"
    );
}

#[test]
fn test_side_panel_header_matches_the_agents_header() {
    let mut app = crate::App::new();
    let rows = render_rows(&mut app, 150);
    let (tabs, widths) = header_bar(&rows, "SBX");
    assert!(
        rows[tabs].contains("PRC") && rows[tabs].contains("│ 🖥  │ * │ ⤢ │"),
        "the tabs, then HOST's icon and the * and ⤢ buttons: {}",
        rows[tabs]
    );
    assert_eq!(
        widths.len(),
        8,
        "the tabs, the icon and the two buttons fill the bar: {widths:?}"
    );
    assert_eq!(widths[5..], [4, 3, 3]);
    assert!(
        widths[..5].iter().all(|w| w.abs_diff(widths[0]) <= 1),
        "{widths:?}"
    );
    let projects = if rows.iter().any(|row| row.contains("PROJECTS")) {
        "PROJECTS"
    } else {
        "PRJ"
    };
    assert_eq!(
        header_bar(&rows, projects).0,
        header_bar(&rows, "KRN").0,
        "top bars line up"
    );

    let rows = render_rows(&mut app, 300);
    let (tabs, widths) = header_bar(&rows, "SANDBOX");
    assert!(rows[tabs].contains("REPO"), "{}", rows[tabs]);
    assert!(rows[tabs].contains("PROCESSES"), "{}", rows[tabs]);
    assert!(!rows[tabs].contains("HOST"), "an icon: {}", rows[tabs]);
    assert_eq!(widths.len(), 8, "{widths:?}");
}

#[test]
fn test_side_panel_star_button_lists_the_whole_project() {
    use crate::app::SideScope;
    use crate::events::UiCommand;

    let mut app = two_agents_with_a_process_each();
    let buffer = render_buffer(&mut app, 150);
    let row = row_of(&buffer, "│ * │");
    let (prc, _) = find_cell(&buffer, row, "PRC").unwrap();
    click(&mut app, prc, row);
    let screen = render_rows(&mut app, 150).join("\n");
    assert!(screen.contains("job-a1"), "{screen}");
    assert!(!screen.contains("job-b1"), "{screen}");

    let buffer = render_buffer(&mut app, 150);
    let (cell, _) = find_cell(&buffer, row, "│ * │").unwrap();
    let all = cell + 2;
    let idle = buffer[(all, row)].style();
    assert_eq!(idle.fg, Some(test_theme().colors.muted));
    click(&mut app, all, row);
    assert_eq!(app.side_scope(), SideScope::Project);
    let buffer = render_buffer(&mut app, 150);
    let lit = buffer[(all, row)].style();
    assert_eq!(buffer[(all, row)].symbol(), SideScope::BUTTON);
    assert!(lit.add_modifier.contains(ratatui::style::Modifier::BOLD));
    let screen = render_rows(&mut app, 150).join("\n");
    assert!(
        screen.contains("job-a1") && screen.contains("job-b1"),
        "{screen}"
    );

    app.process_command(UiCommand::SetActiveAgent("b".into()));
    click(&mut app, all, row);
    let screen = render_rows(&mut app, 150).join("\n");
    assert!(screen.contains("job-b1"), "{screen}");
    assert!(!screen.contains("job-a1"), "{screen}");
}

/// Agents `a` (active) and `b` of one project, each owning one process.
fn two_agents_with_a_process_each() -> crate::App {
    use crate::events::{AgentSummary, ProcessSummary, UiCommand};

    let mut app = crate::App::new();
    let agent = |id: &str| AgentSummary {
        id: id.into(),
        name: id.to_uppercase(),
        project_id: Some("site".into()),
        ..AgentSummary::default()
    };
    app.process_command(UiCommand::SetAgents(vec![agent("a"), agent("b")]));
    app.process_command(UiCommand::SetActiveAgent("a".into()));
    let process = |id: &str, owner: &str| ProcessSummary {
        id: id.into(),
        label: format!("job-{id}"),
        owner_agent_id: owner.into(),
        jump_agent_id: owner.into(),
        ..ProcessSummary::default()
    };
    app.process_command(UiCommand::SetProcesses(vec![
        process("a1", "a"),
        process("b1", "b"),
    ]));
    app
}

fn click(app: &mut crate::App, column: u16, row: u16) {
    use crossterm::event::{MouseButton, MouseEventKind};

    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        mouse_at(app, kind, (column, row));
    }
}

#[test]
fn test_scoped_tabs_say_what_the_star_button_would_list() {
    use crate::events::{AgentSummary, ProcessSummary, UiCommand};

    let mut app = crate::App::new();
    let agent = |id: &str| AgentSummary {
        id: id.into(),
        name: id.to_uppercase(),
        project_id: Some("site".into()),
        ..AgentSummary::default()
    };
    app.process_command(UiCommand::SetAgents(vec![agent("a"), agent("b")]));
    app.process_command(UiCommand::SetActiveAgent("a".into()));
    app.process_command(UiCommand::SetProcesses(vec![ProcessSummary {
        id: "b1".into(),
        owner_agent_id: "b".into(),
        jump_agent_id: "b".into(),
        ..ProcessSummary::default()
    }]));
    app.select_pane_tab(crate::app::PaneTab::Side(
        crate::app::SidePanelTab::Processes,
    ));
    let screen = render_rows(&mut app, 240).join("\n");
    assert!(
        screen.contains("No processes for this agent. * 1."),
        "{screen}"
    );
}

#[test]
fn test_side_panel_tabs_fill_the_bar_without_counts() {
    use crate::app::SidePanelTab;
    use crate::events::{ProcessStatus, ProcessSummary, UiCommand};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::style::Modifier;

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    for file in ["src/main.rs", "src/lib.rs", "Cargo.toml"] {
        std::fs::write(dir.path().join(file), "").unwrap();
    }
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    let process = |id: &str, status| ProcessSummary {
        id: id.into(),
        status,
        ..ProcessSummary::default()
    };
    app.process_command(UiCommand::SetProcesses(vec![
        process("a", ProcessStatus::Running),
        process("b", ProcessStatus::Idle),
        process("c", ProcessStatus::Exited),
    ]));

    let theme = test_theme();
    let buffer = render_buffer(&mut app, 300);
    let row = row_of(&buffer, "PROCESSES");
    let (label, label_style) = find_cell(&buffer, row, "SANDBOX").unwrap();
    assert!(label_style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(label_style.bg, Some(Color::Reset), "active, unfocused");
    assert_eq!(label_style.fg, Some(theme.colors.foreground));
    let screen = render_rows(&mut app, 300).join("\n");
    assert!(!screen.contains("FILES"), "{screen}");

    let (processes, process_style) = find_cell(&buffer, row, "PROCESSES").unwrap();
    assert!(processes > label);
    assert_eq!(process_style.bg, Some(Color::Reset));
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.handle_mouse(MouseEvent {
            kind,
            column: processes,
            row: row - 1,
            modifiers: KeyModifiers::empty(),
        });
    }
    assert_eq!(app.side_tab(), SidePanelTab::Processes);
    let screen = render_rows(&mut app, 300).join("\n");
    assert!(!screen.contains("LIVE"), "{screen}");
}

#[test]
fn test_a_picked_row_is_lit_border_to_border_with_its_text_two_columns_in() {
    use crate::app::{PaneTab, PanelFocus, SidePanelTab};
    use crate::components::ROW_INSET;
    use crate::events::{ProcessStatus, ProcessSummary, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::SetProcesses(vec![ProcessSummary {
        id: "cmd-1".into(),
        label: "cargo build".into(),
        status: ProcessStatus::Exited,
        ..ProcessSummary::default()
    }]));
    app.select_pane_tab(PaneTab::Side(SidePanelTab::Processes));
    assert_eq!(app.focus(), PanelFocus::Sandbox);

    let buffer = render_buffer(&mut app, 175);
    let row = row_of(&buffer, "cargo build");
    let (label, _) = find_cell(&buffer, row, "cargo build").unwrap();
    let border = |x: u16| buffer[(x, row)].symbol() == "│";
    let left = (0..label).rev().find(|&x| border(x)).expect("left border");
    let right = (label..buffer.area.width)
        .find(|&x| border(x))
        .expect("right border");
    let selection = Some(test_theme().colors.selection);
    assert!(
        (left + 1..right).all(|x| buffer[(x, row)].style().bg == selection),
        "lit from border to border"
    );
    let inset = u16::try_from(ROW_INSET).unwrap();
    let blank = |x: u16, y: u16| buffer[(x, y)].symbol() == " ";
    assert!((left + 1..=left + inset).all(|x| blank(x, row)));
    assert!(!blank(left + 1 + inset, row), "the status glyph");
    assert!((right - inset..right).all(|x| blank(x, row)));
    assert!(
        (left + 1..right).all(|x| blank(x, row - 1)),
        "a blank row under the top border"
    );
}
