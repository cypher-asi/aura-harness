use super::*;

#[test]
fn test_agents_header_is_a_bar_of_equal_tabs() {
    use crate::app::AgentsTab;
    let mut app = crate::App::new();
    for width in [120, 160] {
        let rows = render_rows(&mut app, width);
        // A cell too narrow for its label shows the short one.
        let first = if rows.iter().any(|row| row.contains("CHANNELS")) {
            "CHANNELS"
        } else {
            "CHN"
        };
        let (tabs, widths) = header_bar(&rows, first);
        let bar_width = widths.iter().sum::<usize>() + widths.len() + 1;
        let layout = AgentsTab::layout(u16::try_from(bar_width).unwrap());
        let tab_order: Vec<AgentsTab> = layout.cells.iter().map(|cell| cell.tab).collect();
        assert_eq!(tab_order, AgentsTab::ALL);
        let order: Vec<Option<usize>> = layout
            .cells
            .iter()
            .map(|cell| rows[tabs].find(cell.label))
            .collect();
        assert!(order.iter().all(Option::is_some), "{}", rows[tabs]);
        assert!(
            order.windows(2).all(|pair| pair[0] < pair[1]),
            "{}",
            rows[tabs]
        );
        assert_eq!(
            widths.len(),
            AgentsTab::ALL.len(),
            "the tabs fill the bar: {widths:?}"
        );
        let (min, max) = (widths.iter().min(), widths.iter().max());
        assert!(max.zip(min).is_some_and(|(a, b)| a - b <= 1), "{widths:?}");
    }
}

/// Rows of the project list: the first project, its selected agent, the
/// gap, then the next project and its agent.
#[derive(Clone, Copy)]
struct ProjectRows {
    project_row: u16,
    selected_row: u16,
    gap_row: u16,
    researcher_project: u16,
    researcher_agent: u16,
}

/// Two agents in two projects, the first active, focus on the agents
/// panel.
fn project_list_app() -> crate::App {
    use crate::events::{AgentSummary, UiCommand};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let mut app = crate::App::new().with_event_sender(tx);
    let agent = |id: &str, name: &str| AgentSummary {
        id: id.to_string(),
        name: name.to_string(),
        zns_id: format!("0://terminal/{id}"),
        ..AgentSummary::default()
    };
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "Terminal Agent"),
        agent("bb", "Researcher"),
    ]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    for _ in 0..3 {
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
    }
    app
}

fn project_list_rows(app: &mut crate::App) -> ProjectRows {
    use crate::app::ProjectListRow;

    let buffer = render_buffer(app, 120);
    let selected = app.selected_agent();
    assert_eq!(
        app.project_rows()[selected],
        ProjectListRow::Agent { index: 0 }
    );
    // Lines from the first project: the selected agent, the gap, then the
    // next project and its agent.
    let project_row = row_of(&buffer, "TERMINAL AGENT");
    let rows = ProjectRows {
        project_row,
        selected_row: project_row + 1,
        gap_row: project_row + 2,
        researcher_project: project_row + 3,
        researcher_agent: project_row + 4,
    };
    assert!(find_cell(&buffer, rows.selected_row, "Terminal Agent").is_some());
    assert!(find_cell(&buffer, rows.researcher_project, "RESEARCHER").is_some());
    assert!(find_cell(&buffer, rows.researcher_agent, "Researcher").is_some());
    rows
}

fn hover(app: &mut crate::App, row: u16) {
    mouse_at(app, crossterm::event::MouseEventKind::Moved, (4, row));
}

fn check_hover_colors(app: &mut crate::App, r: ProjectRows) {
    let theme = test_theme();
    let colors = &theme.colors;
    hover(app, r.researcher_agent);
    let buffer = render_buffer(app, 120);
    // Hovering only lays a subtle gray under the row; its text keeps its color.
    let (_, hovered) = find_cell(&buffer, r.researcher_agent, "Researcher").unwrap();
    assert_eq!(hovered.fg, Some(colors.foreground));
    assert_eq!(hovered.bg, Some(colors.surface_hover));
    let (_, selected) = find_cell(&buffer, r.selected_row, "Terminal Agent").unwrap();
    assert_eq!(selected.fg, Some(colors.on_selection));
    assert_eq!(selected.bg, Some(colors.selection));
    assert!(!selected.add_modifier.contains(Modifier::BOLD));

    hover(app, r.researcher_project);
    let buffer = render_buffer(app, 120);
    let (_, project) = find_cell(&buffer, r.researcher_project, "RESEARCHER").unwrap();
    assert_eq!(project.bg, Some(colors.surface_hover));
    assert_eq!(project.fg, Some(colors.muted));
    assert!(!project.add_modifier.contains(Modifier::BOLD));
    let (_, agent) = find_cell(&buffer, r.researcher_agent, "Researcher").unwrap();
    assert_eq!(agent.bg, Some(Color::Reset));
}

/// Returns the selected cell's first and last columns.
fn check_selection_spans_the_panel(app: &mut crate::App, r: ProjectRows) -> (u16, u16) {
    use crate::app::ProjectListRow;

    let colors = test_theme().colors;
    // Move off the list so the checks below see no hover.
    hover(app, r.gap_row);
    let buffer = render_buffer(app, 120);

    // The selected cell runs from one panel border to the other; nothing
    // else has a background.
    let left_border = 1;
    assert_eq!(buffer[(left_border, r.selected_row)].symbol(), "│");
    let right_border = (left_border + 1..buffer.area.width)
        .find(|&x| buffer[(x, r.selected_row)].symbol() == "│")
        .unwrap();
    let (left, right) = (left_border + 1, right_border - 1);
    let bg = |x: u16, y: u16| buffer[(x, y)].style().bg;
    for x in left..=right {
        assert_eq!(bg(x, r.selected_row), Some(colors.selection), "column {x}");
        for y in [
            r.project_row,
            r.gap_row,
            r.researcher_project,
            r.researcher_agent,
        ] {
            assert_eq!(bg(x, y), Some(Color::Reset), "column {x}, row {y}");
        }
    }
    assert_eq!(
        app.project_rows()[app.selected_agent() + 1],
        ProjectListRow::Gap
    );
    (left, right)
}

fn check_fold_and_reopen(app: &mut crate::App, r: ProjectRows, left: u16) {
    use crate::app::ProjectListRow;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let colors = test_theme().colors;
    // Up skips the project header: projects are never highlighted, and
    // with no agent above the highlight stays put.
    let cursor = app.selected_agent();
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::empty()));
    assert_eq!(app.selected_agent(), cursor);

    // Folding and reopening the active agent's project highlights neither
    // the project nor anything else, then the agent again.
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::empty()));
    assert!(matches!(
        app.project_rows()[0],
        ProjectListRow::Project {
            expanded: false,
            ..
        }
    ));
    let buffer = render_buffer(app, 120);
    let (_, project) = find_cell(&buffer, r.project_row, "TERMINAL AGENT").unwrap();
    assert_eq!(project.bg, Some(Color::Reset));
    assert_eq!(project.fg, Some(colors.muted));
    assert!((r.project_row..buffer.area.height)
        .all(|y| buffer[(left, y)].style().bg != Some(colors.selection)));
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::empty()));
    assert_eq!(app.selected_agent(), cursor);
    let buffer = render_buffer(app, 120);
    let (_, agent) = find_cell(&buffer, r.selected_row, "Terminal Agent").unwrap();
    assert_eq!(agent.bg, Some(colors.selection));
    let (_, project) = find_cell(&buffer, r.project_row, "TERMINAL AGENT").unwrap();
    assert_eq!(project.bg, Some(Color::Reset));
}

fn check_away_from_the_panel(app: &mut crate::App, r: ProjectRows, right: u16) {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind};

    let colors = test_theme().colors;
    // Away from the panel, the agent shown in the chat stays selected
    // wherever the cursor was. The other rows sit on the panel background:
    // projects gray, agents white, none of them bold.
    mouse_at(app, MouseEventKind::Moved, (right + 10, r.researcher_agent));
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
    let buffer = render_buffer(app, 120);
    let (_, active) = find_cell(&buffer, r.selected_row, "Terminal Agent").unwrap();
    assert_eq!(active.bg, Some(colors.selection));
    assert_eq!(active.fg, Some(colors.on_selection));
    for (row, name, fg) in [
        (r.project_row, "TERMINAL AGENT", colors.muted),
        (r.researcher_project, "RESEARCHER", colors.muted),
        (r.researcher_agent, "Researcher", colors.foreground),
    ] {
        let (_, cell) = find_cell(&buffer, row, name).unwrap();
        assert_eq!(cell.bg, Some(Color::Reset), "{name}");
        assert_eq!(cell.fg, Some(fg), "{name}");
        assert!(!cell.add_modifier.contains(Modifier::BOLD), "{name}");
    }
}

#[test]
fn test_project_rows_are_cells_with_a_blue_selection() {
    let mut app = project_list_app();
    let rows = project_list_rows(&mut app);
    check_hover_colors(&mut app, rows);
    let (left, right) = check_selection_spans_the_panel(&mut app, rows);
    check_fold_and_reopen(&mut app, rows, left);
    check_away_from_the_panel(&mut app, rows, right);
}

#[test]
fn test_active_agent_is_selected_on_open() {
    use crate::events::{AgentSummary, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![
        AgentSummary {
            id: "aa".to_string(),
            name: "Terminal Agent".to_string(),
            ..AgentSummary::default()
        },
        AgentSummary {
            id: "bb".to_string(),
            name: "Researcher".to_string(),
            ..AgentSummary::default()
        },
    ]));
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    assert_ne!(app.focus(), crate::app::PanelFocus::Swarm);

    let buffer = render_buffer(&mut app, 120);
    let colors = test_theme().colors;
    let project = row_of(&buffer, "RESEARCHER");
    let (_, agent) = find_cell(&buffer, project + 1, "Researcher").unwrap();
    assert_eq!(agent.bg, Some(colors.selection));
    let (_, header) = find_cell(&buffer, project, "RESEARCHER").unwrap();
    assert_eq!(header.bg, Some(Color::Reset));
}

#[test]
fn test_hovered_activity_header_keeps_status_color() {
    use crate::events::{ToolData, UiCommand};
    use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};

    let mut app = crate::App::new();
    app.process_command(UiCommand::ShowTool(ToolData {
        id: "t1".to_string(),
        name: "read_file".to_string(),
        args: String::new(),
    }));
    app.process_command(UiCommand::CompleteTool {
        id: "t1".to_string(),
        result: "no such file".to_string(),
        success: false,
    });

    let buffer = render_buffer(&mut app, 120);
    let row = row_of(&buffer, "read_file");
    let (column, _) = find_cell(&buffer, row, "read_file").unwrap();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    });

    let theme = test_theme();
    let buffer = render_buffer(&mut app, 120);
    let (_, name) = find_cell(&buffer, row, "read_file").unwrap();
    assert_eq!(name.fg, Some(theme.colors.secondary));
    assert!(!name.add_modifier.contains(Modifier::UNDERLINED));
    let (_, mark) = find_cell(&buffer, row, "✗").unwrap();
    assert_eq!(mark.fg, Some(theme.colors.error));
}
