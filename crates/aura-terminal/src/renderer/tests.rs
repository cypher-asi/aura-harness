use super::markdown::{parse_content_segments, ContentSegment};
use super::panels::truncate_record_info;
use super::text::{centered_rect, display_width, wrap_words};
use crate::events::CallUsage;
use crate::Theme;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
};

fn test_theme() -> Theme {
    Theme::cyber()
}

// ========================================================================
// Word Wrapping Tests
// ========================================================================

#[test]
fn test_wrap_words_simple() {
    let result = wrap_words("hello world", 20);
    assert_eq!(result, vec!["hello world"]);
}

#[test]
fn test_wrap_words_wraps_at_boundary() {
    let result = wrap_words("hello world foo bar", 11);
    assert_eq!(result, vec!["hello world", "foo bar"]);
}

#[test]
fn test_wrap_words_empty_input() {
    let result = wrap_words("", 20);
    assert_eq!(result, vec![""]);
}

#[test]
fn test_wrap_words_whitespace_only() {
    let result = wrap_words("   ", 20);
    assert_eq!(result, vec![""]);
}

#[test]
fn test_wrap_words_long_word() {
    let result = wrap_words("supercalifragilistic", 10);
    assert_eq!(result.len(), 2);
    assert!(result[0].len() <= 10);
}

#[test]
fn test_wrap_words_zero_width() {
    let result = wrap_words("hello", 0);
    assert_eq!(result, vec!["hello"]);
}

#[test]
fn test_wrap_words_exact_fit() {
    let result = wrap_words("hello", 5);
    assert_eq!(result, vec!["hello"]);
}

// ========================================================================
// Display Width Tests
// ========================================================================

#[test]
fn test_display_width_ascii() {
    assert_eq!(display_width("hello"), 5);
    assert_eq!(display_width(""), 0);
}

#[test]
fn test_display_width_unicode() {
    let width = display_width("你好");
    assert!(width >= 2);
}

#[test]
fn test_display_width_emoji() {
    let width = display_width("🎉");
    assert!(width >= 1);
}

// ========================================================================
// Content Segment Parsing Tests
// ========================================================================

#[test]
fn test_parse_content_segments_text_only() {
    let segments = parse_content_segments("Hello\nWorld");
    assert_eq!(segments.len(), 1);
    assert!(matches!(&segments[0], ContentSegment::Text(_)));
}

#[test]
fn test_parse_content_segments_code_block() {
    let content = "Before\n```rust\nfn main() {}\n```\nAfter";
    let segments = parse_content_segments(content);

    assert_eq!(segments.len(), 3);
    assert!(matches!(&segments[0], ContentSegment::Text(_)));
    assert!(matches!(&segments[1], ContentSegment::CodeBlock { .. }));
    assert!(matches!(&segments[2], ContentSegment::Text(_)));

    if let ContentSegment::CodeBlock { language, code } = &segments[1] {
        assert_eq!(language, "rust");
        assert!(code.contains("fn main()"));
    }
}

#[test]
fn test_parse_content_segments_multiple_code_blocks() {
    let content = "```python\nprint('hello')\n```\ntext\n```js\nconsole.log('hi')\n```";
    let segments = parse_content_segments(content);

    assert_eq!(segments.len(), 3);
    assert!(matches!(&segments[0], ContentSegment::CodeBlock { .. }));
    assert!(matches!(&segments[1], ContentSegment::Text(_)));
    assert!(matches!(&segments[2], ContentSegment::CodeBlock { .. }));
}

#[test]
fn test_parse_content_segments_unclosed_code_block() {
    let content = "Before\n```rust\nfn main() {}";
    let segments = parse_content_segments(content);

    assert!(!segments.is_empty());
    for segment in &segments {
        assert!(matches!(segment, ContentSegment::Text(_)));
    }
}

// ========================================================================
// Centered Rect Tests
// ========================================================================

#[test]
fn test_centered_rect_normal() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 50,
    };
    let result = centered_rect(20, 10, area);

    assert_eq!(result.width, 20);
    assert_eq!(result.height, 10);
    assert_eq!(result.x, 40);
    assert_eq!(result.y, 20);
}

#[test]
fn test_centered_rect_larger_than_area() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 50,
        height: 30,
    };
    let result = centered_rect(100, 50, area);

    assert_eq!(result.width, 50);
    assert_eq!(result.height, 30);
}

#[test]
fn test_centered_rect_with_offset() {
    let area = Rect {
        x: 10,
        y: 5,
        width: 100,
        height: 50,
    };
    let result = centered_rect(20, 10, area);

    assert_eq!(result.x, 50);
    assert_eq!(result.y, 25);
}

// ========================================================================
// Layout
// ========================================================================

#[test]
fn test_default_layout_shows_agents_column() {
    use crate::events::{AgentDefinitionSummary, AgentSummary, UiCommand};

    let now_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![
        AgentSummary {
            id: "aa".to_string(),
            name: "Terminal Agent".to_string(),
            zns_id: "0://terminal/aa".to_string(),
            is_active: true,
            record_count: 12,
            last_active: "-".to_string(),
            last_active_ms: Some(now_ms - 3 * 60 * 1000),
            ..Default::default()
        },
        AgentSummary {
            id: "bb".to_string(),
            name: "Researcher".to_string(),
            zns_id: "0://terminal/bb".to_string(),
            is_active: false,
            record_count: 0,
            last_active: "-".to_string(),
            running: true,
            ..Default::default()
        },
        AgentSummary {
            id: "cc".to_string(),
            name: "reviewer #1".to_string(),
            zns_id: "0://terminal/cc".to_string(),
            definition: Some("reviewer".to_string()),
            depth: 1,
            ..Default::default()
        },
    ]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(UiCommand::SetAgentDefinitions(vec![
        AgentDefinitionSummary {
            name: "reviewer".to_string(),
            description: "Reviews diffs".to_string(),
        },
    ]));

    let rows = render_rows(&mut app, 200);
    let screen = rows.join("\n");
    assert!(!screen.contains("┌ AGENTS"));
    assert!(screen.contains("CHAT"));
    assert!(screen.contains("KERNEL"));
    assert!(rows
        .iter()
        .any(|r| r.contains("PROJECTS") && r.contains("AGENTS")));
    assert!(!screen.contains("←→"));
    let char_col = |row: &str, needle: &str| {
        let i = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
        row[..i].chars().count()
    };
    let rows_with = |needle: &str| -> Vec<usize> {
        rows.iter()
            .enumerate()
            .filter(|(_, r)| r.contains(needle))
            .map(|(i, _)| i)
            .collect()
    };
    // A project row and its first agent row share each name.
    let [terminal_project, terminal_agent] = rows_with("Terminal Agent")[..] else {
        panic!("expected a project row and an agent row for Terminal Agent");
    };
    let [researcher_project, researcher_agent] = rows_with("Researcher")[..] else {
        panic!("expected a project row and an agent row for Researcher");
    };
    let new_project = rows_with("New project")[0];
    assert!(rows[terminal_project].contains("▱  Terminal Agent"));
    // An idle agent leaves the icon column blank, even when it is active.
    let agent_panel: String = rows[terminal_agent].chars().take(30).collect();
    assert!(
        !agent_panel.contains('●') && !agent_panel.contains('·'),
        "{agent_panel:?}"
    );
    assert_eq!(
        char_col(&rows[terminal_agent], "Terminal Agent"),
        char_col(&rows[terminal_project], "Terminal Agent")
    );
    assert!(rows[terminal_agent].contains(" 3m "));
    assert!(rows[researcher_project].contains("▱  Researcher"));
    assert!(!screen.contains("12│"));
    assert!(!screen.contains("# ") && !screen.contains("@ "));
    assert!(!screen.contains('↳') && !screen.contains('▾') && !screen.contains('▸'));

    // A thinking agent shows the two-cell aura ring in the icon column.
    let ring_col = char_col(&rows[terminal_project], "▱");
    let ring: String = rows[researcher_agent]
        .chars()
        .skip(ring_col)
        .take(crate::animation::AURA_WIDTH)
        .collect();
    assert!(
        ring.chars().all(|c| ('\u{2800}'..='\u{28ff}').contains(&c))
            && ring.chars().any(|c| c != '\u{2800}'),
        "expected a braille ring, got {ring:?}"
    );

    // Icons and names line up down the list, with a blank row between projects.
    assert_eq!(
        char_col(&rows[terminal_project], "▱"),
        char_col(&rows[new_project], "+")
    );
    assert_eq!(ring_col, char_col(&rows[terminal_project], "▱"));
    assert_eq!(
        char_col(&rows[new_project], "New project"),
        char_col(&rows[researcher_project], "Researcher")
    );
    // Projects and agents are one line each, with one blank line between projects.
    assert_eq!(terminal_agent, terminal_project + 1);
    assert_eq!(researcher_project, terminal_agent + 2);
    let panel_text = |row: usize| rows[row].chars().skip(1).take(20).collect::<String>();
    assert_eq!(panel_text(terminal_agent + 1).trim(), "");
    assert_eq!(researcher_agent, researcher_project + 1);
    // "+ New project" takes the panel's bottom row in place of a key hint.
    assert!(!screen.contains("space fold") && !screen.contains("^N new"));
    assert!(
        rows[new_project + 1].starts_with('└'),
        "New project should sit on the panel's last row: {:?}",
        &rows[new_project..=new_project + 1]
    );
    assert!(new_project > rows_with("reviewer #1")[0] + 1);
    // Spawned agents sit further in than the project's first agent.
    assert!(
        char_col(&rows[rows_with("reviewer #1")[0]], "reviewer #1")
            > char_col(&rows[researcher_agent], "Researcher")
    );
    assert!(!screen.contains("Reviews"));

    // Tab to the agents panel. Neither the selected row nor any other
    // shows a cursor arrow.
    for _ in 0..3 {
        app.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Tab,
        ));
    }
    let rows = render_rows(&mut app, 200);
    let panel: Vec<String> = rows.iter().map(|r| r.chars().take(30).collect()).collect();
    assert!(
        panel.iter().all(|r| !r.contains('>')),
        "projects list should not show a cursor arrow: {panel:#?}"
    );
    app.handle_key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Right,
    ));
    let rows = render_rows(&mut app, 120);
    let screen = rows.join("\n");
    assert!(screen.contains("@  reviewer"));
    assert!(!screen.contains("Reviews"));
    assert!(screen.contains("+  New agent"));
    let column_of = |needle: &str| {
        rows.iter()
            .find_map(|r| r.find(needle).map(|i| r[..i].chars().count()))
            .unwrap()
    };
    assert_eq!(column_of("@"), column_of("+"));
    assert_eq!(column_of("reviewer"), column_of("New agent"));
    assert!(!screen.contains("spawn") && !screen.contains(" del"));
    assert!(!screen.contains("Researcher"));
    // "+ New agent" takes the panel's bottom row, like "+ New project".
    let new_agent = rows.iter().position(|r| r.contains("New agent")).unwrap();
    assert!(
        rows[new_agent + 1].starts_with('└'),
        "New agent should sit on the panel's last row: {:?}",
        &rows[new_agent..=new_agent + 1]
    );
    assert!(new_agent > rows.iter().position(|r| r.contains("@  reviewer")).unwrap() + 1);
}

#[test]
fn test_format_ago_steps_up_at_each_unit() {
    use super::panels::format_ago;
    const MIN: u64 = 60_000;
    let now = 100 * 7 * 24 * 60 * MIN;
    let ago = |ms: u64| format_ago(now - ms, now);
    assert_eq!(ago(0), "now");
    assert_eq!(ago(MIN - 1), "now");
    assert_eq!(ago(MIN), "1m");
    assert_eq!(ago(60 * MIN - 1), "59m");
    assert_eq!(ago(60 * MIN), "1h");
    assert_eq!(ago(24 * 60 * MIN), "1d");
    assert_eq!(ago(7 * 24 * 60 * MIN - 1), "6d");
    assert_eq!(ago(7 * 24 * 60 * MIN), "1w");
    assert_eq!(format_ago(now + MIN, now), "now");
}

#[test]
fn test_agents_tabs_are_one_line_then_a_blank_line() {
    let mut app = crate::App::new();
    let rows = render_rows(&mut app, 120);
    let grid: Vec<Vec<char>> = rows.iter().map(|r| r.chars().collect()).collect();
    let tabs = rows.iter().position(|r| r.contains("PROJECTS")).unwrap();
    let panel_left = grid[tabs].iter().position(|&c| c == '│').unwrap();
    let panel_right = panel_left
        + 1
        + grid[tabs][panel_left + 1..]
            .iter()
            .position(|&c| c == '│')
            .unwrap();

    // Right under the panel's top border, with nothing drawn around them.
    assert_eq!(grid[tabs - 1][panel_left], '┌');
    let labels: String = grid[tabs][panel_left + 1..panel_right].iter().collect();
    assert_eq!(labels.trim_end(), " PROJECTS  AGENTS");
    assert!(grid[tabs + 1][panel_left + 1..panel_right]
        .iter()
        .all(|&c| c == ' '));
}

#[test]
fn test_project_rows_are_cells_with_a_blue_selection() {
    use crate::app::ProjectListRow;
    use crate::events::{AgentSummary, UiCommand};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

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

    let buffer = render_buffer(&mut app, 120);
    let selected = app.selected_agent();
    assert_eq!(
        app.project_rows()[selected],
        ProjectListRow::Agent { index: 0 }
    );
    // Lines from the first project: the selected agent, the gap, then the
    // next project and its agent.
    let project_row = row_of(&buffer, "Terminal Agent");
    let selected_row = project_row + 1;
    let gap_row = project_row + 2;
    let researcher_project = project_row + 3;
    let researcher_agent = project_row + 4;
    assert!(find_cell(&buffer, selected_row, "Terminal Agent").is_some());
    assert!(find_cell(&buffer, researcher_project, "Researcher").is_some());
    assert!(find_cell(&buffer, researcher_agent, "Researcher").is_some());
    let hover = |app: &mut crate::App, row: u16| {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 4,
            row,
            modifiers: KeyModifiers::empty(),
        });
    };

    let theme = test_theme();
    let colors = &theme.colors;
    hover(&mut app, researcher_agent);
    let buffer = render_buffer(&mut app, 120);
    // Hovering only lays a subtle gray under the row; its text keeps its color.
    let (_, hovered) = find_cell(&buffer, researcher_agent, "Researcher").unwrap();
    assert_eq!(hovered.fg, Some(colors.foreground));
    assert_eq!(hovered.bg, Some(colors.surface_hover));
    let (_, selected) = find_cell(&buffer, selected_row, "Terminal Agent").unwrap();
    assert_eq!(selected.fg, Some(colors.on_selection));
    assert_eq!(selected.bg, Some(colors.selection));
    assert!(!selected.add_modifier.contains(Modifier::BOLD));

    hover(&mut app, researcher_project);
    let buffer = render_buffer(&mut app, 120);
    let (_, project) = find_cell(&buffer, researcher_project, "Researcher").unwrap();
    assert_eq!(project.bg, Some(colors.surface_hover));
    assert_eq!(project.fg, Some(colors.muted));
    assert!(project.add_modifier.contains(Modifier::BOLD));
    let (_, agent) = find_cell(&buffer, researcher_agent, "Researcher").unwrap();
    assert_eq!(agent.bg, Some(Color::Reset));

    // Move off the list so the checks below see no hover.
    hover(&mut app, gap_row);
    let buffer = render_buffer(&mut app, 120);

    // The selected cell runs from one panel border to the other; nothing
    // else has a background.
    let right_border = (1..buffer.area.width)
        .find(|&x| buffer[(x, selected_row)].symbol() == "│")
        .unwrap();
    let (left, right) = (1, right_border - 1);
    let bg = |x: u16, y: u16| buffer[(x, y)].style().bg;
    for x in left..=right {
        assert_eq!(bg(x, selected_row), Some(colors.selection), "column {x}");
        for y in [project_row, gap_row, researcher_project, researcher_agent] {
            assert_eq!(bg(x, y), Some(Color::Reset), "column {x}, row {y}");
        }
    }
    assert_eq!(
        app.project_rows()[app.selected_agent() + 1],
        ProjectListRow::Gap
    );

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
    let buffer = render_buffer(&mut app, 120);
    let (_, project) = find_cell(&buffer, project_row, "Terminal Agent").unwrap();
    assert_eq!(project.bg, Some(Color::Reset));
    assert_eq!(project.fg, Some(colors.muted));
    assert!((0..buffer.area.height).all(|y| buffer[(left, y)].style().bg != Some(colors.selection)));
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::empty()));
    assert_eq!(app.selected_agent(), cursor);
    let buffer = render_buffer(&mut app, 120);
    let (_, agent) = find_cell(&buffer, selected_row, "Terminal Agent").unwrap();
    assert_eq!(agent.bg, Some(colors.selection));
    let (_, project) = find_cell(&buffer, project_row, "Terminal Agent").unwrap();
    assert_eq!(project.bg, Some(Color::Reset));

    // Away from the panel, the agent shown in the chat stays selected
    // wherever the cursor was. The other rows sit on the panel background:
    // projects bold gray, agents plain white.
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: right + 10,
        row: researcher_agent,
        modifiers: KeyModifiers::empty(),
    });
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
    let buffer = render_buffer(&mut app, 120);
    let (_, active) = find_cell(&buffer, selected_row, "Terminal Agent").unwrap();
    assert_eq!(active.bg, Some(colors.selection));
    assert_eq!(active.fg, Some(colors.on_selection));
    for (row, name, fg, bold) in [
        (project_row, "Terminal Agent", colors.muted, true),
        (researcher_project, "Researcher", colors.muted, true),
        (researcher_agent, "Researcher", colors.foreground, false),
    ] {
        let (_, cell) = find_cell(&buffer, row, name).unwrap();
        assert_eq!(cell.bg, Some(Color::Reset), "{name}");
        assert_eq!(cell.fg, Some(fg), "{name}");
        assert_eq!(cell.add_modifier.contains(Modifier::BOLD), bold, "{name}");
    }
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
    let project = row_of(&buffer, "Researcher");
    let (_, agent) = find_cell(&buffer, project + 1, "Researcher").unwrap();
    assert_eq!(agent.bg, Some(colors.selection));
    let (_, header) = find_cell(&buffer, project, "Researcher").unwrap();
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

#[test]
fn test_chat_renders_assistant_markdown() {
    use crate::components::{Message, MessageRole};

    let mut app = crate::App::new();
    app.add_message(Message::new(
        MessageRole::Assistant,
        "## Summary\n\nThe build is **green** and `cargo test` passes.\n\n\
         - first point that is long enough to wrap onto a second line in the chat panel\n\
         - [x] done\n\n\
         | Crate | Tests |\n|---|---:|\n| aura-terminal | 231 |\n\n\
         > note: see [docs](https://example.com)",
    ));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    terminal
        .draw(|frame| super::render(frame, &mut app, &test_theme()))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let rows: Vec<String> = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect();
    let screen = rows.join("\n");
    let header = rows.iter().position(|r| r.contains("AURA  ")).unwrap();
    assert!(rows[header + 1].contains("Summary"), "{screen}");
    assert!(!screen.contains("##") && !screen.contains("**"), "{screen}");
    assert!(screen.contains("The build is green and cargo"), "{screen}");
    assert!(screen.contains("• [✓] done"), "{screen}");
    assert!(screen.contains("│ aura-terminal │   231 │"), "{screen}");
    assert!(screen.contains("│ note: see docs"), "{screen}");
    assert!(screen.contains("│ (https://example.com)"), "{screen}");

    let bullet_row = rows.iter().find(|r| r.contains("• first point")).unwrap();
    let bullet_col = bullet_row.chars().position(|c| c == '•').unwrap();
    let next_row = &rows[rows.iter().position(|r| r == bullet_row).unwrap() + 1];
    let hang_col = next_row.chars().position(|c| c.is_alphanumeric()).unwrap();
    assert_eq!(
        hang_col,
        bullet_col + 2,
        "wrapped item hangs under its text"
    );
}

#[test]
fn test_chat_names_sit_above_centered_text() {
    use crate::components::{Message, MessageRole};

    let mut app = crate::App::new();
    app.add_message(Message::new(MessageRole::User, "hi"));
    app.add_message(Message::new(
        MessageRole::Assistant,
        "The task of extracting tasks from a spec file is not currently defined \
         or available in the current context.",
    ));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
    terminal
        .draw(|frame| super::render(frame, &mut app, &test_theme()))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let rows: Vec<String> = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect();
    let col = |row: &str, needle: &str| {
        let i = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
        row[..i].chars().count()
    };
    let user = rows.iter().position(|r| r.contains("YOU  ")).unwrap();
    let aura = rows.iter().position(|r| r.contains("AURA  ")).unwrap();

    let text_col = col(&rows[user], "YOU");
    assert_eq!(col(&rows[user + 1], "hi"), text_col);
    assert_eq!(col(&rows[aura], "AURA"), text_col);
    assert_eq!(col(&rows[aura + 1], "The task"), text_col);
    assert_eq!(rows[aura + 2].chars().nth(text_col - 1), Some(' '));
    assert_ne!(rows[aura + 2].chars().nth(text_col), Some(' '));
}

#[test]
fn test_chat_text_is_centered_with_margins() {
    use super::panels::centered_text_area;

    let gaps = |area: Rect| {
        let text = centered_text_area(area);
        (
            text.x - area.x,
            (area.x + area.width) - (text.x + text.width),
        )
    };
    assert_eq!(gaps(Rect::new(10, 0, 60, 20)), (4, 4));
    let (left, right) = gaps(Rect::new(10, 0, 61, 20));
    assert!(left.abs_diff(right) <= 1);
    let wide = Rect::new(0, 0, 200, 20);
    assert_eq!(centered_text_area(wide).width, 88);
    assert_eq!(gaps(wide), (56, 56));
}

fn submit(app: &mut crate::App, text: &str) {
    use crossterm::event::{KeyCode, KeyEvent};
    for c in text.chars() {
        let _ = app.handle_key(KeyEvent::from(KeyCode::Char(c)));
    }
    let _ = app.handle_key(KeyEvent::from(KeyCode::Enter));
}

#[test]
fn test_sandbox_panel_sits_below_record() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();

    let mut app = crate::App::new();
    app.process_command(crate::events::UiCommand::SetSandboxRoot(
        dir.path().to_path_buf(),
    ));
    let rows = render_rows(&mut app, 120);

    let record = rows.iter().position(|r| r.contains(" KERNEL ")).unwrap();
    let sandbox = rows.iter().position(|r| r.contains(" SANDBOX ")).unwrap();
    assert!(sandbox > record + 10, "record={record} sandbox={sandbox}");
    let column = |row: &str, title: &str| row[..row.find(title).unwrap()].chars().count();
    assert_eq!(
        column(&rows[record], " KERNEL "),
        column(&rows[sandbox], " SANDBOX ")
    );
    assert!(rows[sandbox].contains(" PROCESSES "));
    assert!(rows[sandbox + 2].contains("▸ src/"));
    assert!(rows[sandbox + 3].contains("Cargo.toml"));
    assert_eq!(
        column(&rows[sandbox + 2], "▸ src/"),
        column(&rows[sandbox], " SANDBOX ") + 1,
        "tree rows share the Record panel's one-column `>` gutter"
    );
}

#[test]
fn test_side_panel_tabs_match_the_agents_tabs() {
    let mut app = crate::App::new();
    let rows = render_rows(&mut app, 120);
    let grid: Vec<Vec<char>> = rows.iter().map(|r| r.chars().collect()).collect();
    let tabs = rows.iter().position(|r| r.contains(" SANDBOX ")).unwrap();
    let start = rows[tabs][..rows[tabs].find(" SANDBOX ").unwrap()]
        .chars()
        .count();
    assert_eq!(grid[tabs - 1][start - 1], '┌');
    let labels: String = grid[tabs][start..].iter().collect();
    assert!(labels.starts_with(" SANDBOX  PROCESSES "), "{labels}");
    assert!(grid[tabs + 1][start..start + 20].iter().all(|&c| c == ' '));
}

#[test]
fn test_side_panel_tabs_count_files_and_running_processes() {
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

    let buffer = render_buffer(&mut app, 120);
    let row = row_of(&buffer, " SANDBOX 3  PROCESSES 2 ");
    let (label, label_style) = find_cell(&buffer, row, "SANDBOX").unwrap();
    let (_, count_style) = find_cell(&buffer, row, "3  PROC").unwrap();
    assert!(label_style.add_modifier.contains(Modifier::BOLD));
    assert!(!count_style.add_modifier.contains(Modifier::BOLD));

    let (processes, _) = find_cell(&buffer, row, "PROCESSES 2").unwrap();
    assert!(processes > label);
    let count = processes + 10;
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.handle_mouse(MouseEvent {
            kind,
            column: count,
            row,
            modifiers: KeyModifiers::empty(),
        });
    }
    assert_eq!(app.side_tab(), SidePanelTab::Processes);
}

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
    let screen = render_rows(&mut app, 140).join("\n");
    let row = screen.lines().find(|r| r.contains("cargo build")).unwrap();
    assert!(row.contains("command"), "{row}");
    assert!(screen
        .lines()
        .any(|r| r.contains("Agent") && r.contains("turn")));
}

#[test]
fn test_files_command_toggles_sandbox_panel() {
    let mut app = crate::App::new();
    submit(&mut app, "/kernel");
    submit(&mut app, "/files");
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(!screen.contains(" KERNEL "));
    assert!(!screen.contains(" SANDBOX "));

    submit(&mut app, "/files");
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(screen.contains(" SANDBOX "));
    assert!(!screen.contains(" KERNEL "));
}

fn render_with_user(user: Option<&str>, width: u16) -> Vec<String> {
    render_rows(&mut app_with_model(user), width)
}

fn app_with_model(user: Option<&str>) -> crate::App {
    app_with_auth(user, &[])
}

fn app_with_auth(user: Option<&str>, plans: &[&str]) -> crate::App {
    use crate::events::{ModelOptionView, UiCommand};

    let mut app = crate::App::new();
    app.set_shell_prompt("[n3o@arch-aura aura-harness]$ ");
    app.process_command(UiCommand::SetModels {
        options: vec![ModelOptionView {
            id: "local/qwen3:4b".to_string(),
            label: "qwen3:4b".to_string(),
            local: true,
            installed: true,
            group: "Local (no login)".to_string(),
            provider: None,
            available: true,
        }],
        current: "local/qwen3:4b".to_string(),
        user: user.map(str::to_string),
        plans: plans.iter().map(|p| (*p).to_string()).collect(),
    });
    app.process_command(UiCommand::SetApiStatus {
        url: Some("http://127.0.0.1:8080".to_string()),
        active: true,
    });
    app
}

#[test]
fn test_folder_picker_overlay_lists_the_launch_directory() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = crate::App::new();
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    let rows = render_rows(&mut app, 120);
    let screen = rows.join("\n");
    assert!(screen.contains("Select project folder"));
    assert!(screen.contains("› ✓ Use this folder"));
    assert!(screen.contains("▰ src"));
    assert!(screen.contains("n new folder"));

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    for c in "demo".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(screen.contains("New folder: demo"));
    assert!(screen.contains("Enter create  Esc stop"));
}

fn render_rows(app: &mut crate::App, width: u16) -> Vec<String> {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30)).unwrap();
    terminal
        .draw(|frame| super::render(frame, app, &test_theme()))
        .unwrap();

    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

#[test]
fn test_slash_menu_spans_chat_width_above_input() {
    let mut app = app_with_model(None);
    type_text(&mut app, "/lo");
    let rows = render_rows(&mut app, 120);
    let n = rows.len();

    let title = rows.iter().find(|r| r.contains("┌ CHAT ")).unwrap();
    let chat_left = title[..title.find("┌ CHAT ").unwrap()].chars().count();
    let chat_right = chat_left
        + 1
        + title
            .chars()
            .skip(chat_left + 1)
            .position(|c| c == '┐')
            .unwrap();

    let menu_bottom = &rows[n - 3];
    let chars: Vec<char> = menu_bottom.chars().collect();
    assert_eq!(chars[chat_left], '└', "{menu_bottom:?}");
    assert_eq!(chars[chat_right], '┘', "{menu_bottom:?}");
    assert!(menu_bottom.contains("Esc close"), "{menu_bottom:?}");

    assert!(rows[n - 5].contains("› /login"), "{:?}", rows[n - 5]);
    assert!(rows[n - 5].contains("Login for cloud models"));
    assert!(rows[n - 4].contains("  /logout"), "{:?}", rows[n - 4]);
    assert!(rows[n - 6].contains(" COMMANDS "), "{:?}", rows[n - 6]);
    let menu_top: Vec<char> = rows[n - 6].chars().collect();
    assert_eq!(menu_top[chat_left], '┌');
    assert_eq!(menu_top[chat_right], '┐');

    type_text(&mut app, " ");
    let rows = render_rows(&mut app, 120);
    assert!(!rows.join("\n").contains(" COMMANDS "));
}

#[test]
fn test_agent_space_shows_permanent_agents_to_add() {
    use crate::events::{AgentDefinitionSummary, UiCommand};

    let mut app = app_with_model(None);
    app.process_command(UiCommand::SetAgentDefinitions(vec![
        AgentDefinitionSummary {
            name: "explorer".to_string(),
            description: "Reads code\nand more".to_string(),
        },
        AgentDefinitionSummary {
            name: "reviewer".to_string(),
            description: "Reviews diffs".to_string(),
        },
    ]));
    type_text(&mut app, "/agent ");
    let rows = render_rows(&mut app, 120);
    let n = rows.len();

    assert!(
        rows[n - 6].contains(" ADD AGENT TO PROJECT "),
        "{:?}",
        rows[n - 6]
    );
    assert!(
        rows[n - 5].contains("› explorer   Reads code"),
        "{:?}",
        rows[n - 5]
    );
    assert!(!rows[n - 5].contains("and more"));
    assert!(
        rows[n - 4].contains("  reviewer   Reviews diffs"),
        "{:?}",
        rows[n - 4]
    );
    assert!(rows[n - 3].contains("Enter add"), "{:?}", rows[n - 3]);
}

#[test]
fn test_header_shows_local_url_and_model_moves_to_input() {
    let rows = render_with_user(None, 120);
    assert!(rows[1]
        .trim_end()
        .ends_with("● local  http://127.0.0.1:8080"));
    let header = rows[..3].join("\n");
    assert!(!header.contains("qwen3:4b"));

    let n = rows.len();
    assert!(rows[n - 3].contains('└'), "panel border above input");
    assert!(
        rows[n - 1].trim().is_empty(),
        "blank row below input: {:?}",
        rows[n - 1]
    );
    let input_row = rows[n - 2].trim_end();
    assert!(input_row.contains("> "), "{input_row:?}");
    assert!(!input_row.contains("]$ "), "{input_row:?}");
    assert!(input_row.ends_with("qwen3:4b  ● Ready"), "{input_row:?}");
    assert!(!input_row.contains("/model"));
}

fn type_text(app: &mut crate::App, text: &str) {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    for c in text.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
    }
}

#[test]
fn test_prompt_colors_user_green_and_host_gray() {
    let theme = test_theme();
    let cyan = theme.colors.primary;
    let spans = super::input::prompt_spans("[n3o@arch-aura aura-harness]$ ", cyan, &theme);
    let parts: Vec<(&str, Option<Color>)> = spans
        .iter()
        .map(|s| (s.content.as_ref(), s.style.fg))
        .collect();
    assert_eq!(
        parts,
        [
            ("[", Some(cyan)),
            ("n3o", Some(theme.colors.success)),
            ("@arch-aura", Some(theme.colors.muted)),
            (" aura-harness]$ ", Some(cyan)),
        ]
    );

    let email = super::input::prompt_spans("Email: ", cyan, &theme);
    assert_eq!(email.len(), 1);
    assert_eq!(email[0].style.fg, Some(cyan));
}

#[test]
fn test_typing_starts_at_chat_left_border_with_prompt_under_agents() {
    let mut app = app_with_model(None);
    type_text(&mut app, "hello");
    let rows = render_rows(&mut app, 120);
    let input_row = &rows[rows.len() - 2];

    let chat_border = rows
        .iter()
        .find_map(|r| {
            r.find("┌ CHAT")
                .or_else(|| r.find("╭ CHAT"))
                .map(|i| r[..i].chars().count())
        })
        .expect("chat title row");
    let chars: Vec<char> = input_row.chars().collect();
    let typing = chat_border + 1;
    let typed: String = chars[typing..typing + 5].iter().collect();
    let prompt: String = chars[typing - 2..typing].iter().collect();
    assert_eq!(typed, "hello", "{input_row:?}");
    assert_eq!(prompt, "> ", "{input_row:?}");
    assert!(
        chars[..typing - 2].iter().all(|c| *c == ' '),
        "{input_row:?}"
    );
}

#[test]
fn test_prompt_pushes_typing_right_without_gutter() {
    let mut app = app_with_model(None);
    submit_message(&mut app, "/swarm");
    type_text(&mut app, "hello");
    let rows = render_rows(&mut app, 120);
    let input_row = &rows[rows.len() - 2];

    assert!(input_row.starts_with(" > hello"), "{input_row:?}");
}

/// Draw `app` and return the screen rows and where the cursor is.
fn render_with_cursor(app: &mut crate::App, width: u16) -> (Vec<String>, (u16, u16)) {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30)).unwrap();
    terminal
        .draw(|frame| super::render(frame, app, &test_theme()))
        .unwrap();
    let cursor = terminal.get_cursor_position().unwrap();
    let buffer = terminal.backend().buffer();
    let rows = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect();
    (rows, (cursor.x, cursor.y))
}

#[test]
fn test_terminal_types_at_a_prompt_inside_the_pane() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = app_with_model(None);
    type_text(&mut app, "$ls");
    let (rows, (x, y)) = render_with_cursor(&mut app, 120);

    let title = rows.iter().position(|r| r.contains(" TERMINAL ")).unwrap();
    let first = &rows[title + 1];
    let prompt_at = first
        .find("│ [n3o@arch-aura aura-harness]$ ls")
        .map_or_else(
            || panic!("prompt on the pane's first row: {first:?}"),
            |i| first[..i].chars().count(),
        );
    let typed = prompt_at + 2 + "[n3o@arch-aura aura-harness]$ ls".len();
    assert_eq!((usize::from(x), usize::from(y)), (typed, title + 1));

    let input_row = &rows[rows.len() - 2];
    assert!(input_row.contains("Esc returns to chat"), "{input_row:?}");
    assert!(
        !input_row.contains("> ") && !input_row.contains("]$"),
        "{input_row:?}"
    );

    app.handle_key(KeyEvent::from(KeyCode::Backspace));
    app.handle_key(KeyEvent::from(KeyCode::Backspace));
    app.handle_key(KeyEvent::from(KeyCode::Backspace));
    let rows = render_rows(&mut app, 120);
    let input_row = &rows[rows.len() - 2];
    assert!(input_row.contains("> "), "{input_row:?}");
    assert!(!rows.join("\n").contains("]$ "));
}

#[test]
fn test_long_command_wraps_at_the_last_column() {
    let mut app = app_with_model(None);
    app.set_shell_prompt("$ ");
    submit_message(&mut app, "/swarm");
    let command = format!("echo {}", "x".repeat(200));
    type_text(&mut app, &format!("${command}"));
    let (rows, (x, y)) = render_with_cursor(&mut app, 120);

    let start = rows.iter().position(|r| r.contains("│ $ echo x")).unwrap();
    let chars: Vec<char> = rows[start].chars().collect();
    let left = rows[start]
        .find("│ $ echo")
        .map(|i| rows[start][..i].chars().count());
    let left = left.unwrap();
    let right = left + 1 + chars[left + 1..].iter().position(|c| *c == '│').unwrap();
    let (text_x, text_width) = (left + 2, right - left - 3);
    let shown: String = rows[start..]
        .iter()
        .take_while(|r| r.contains('x'))
        .map(|r| r.chars().skip(text_x).take(text_width).collect::<String>())
        .collect();
    assert_eq!(shown.trim_end(), format!("$ {command}"), "wrapped mid-word");
    let typed = 2 + command.len();
    assert_eq!(
        (usize::from(x), usize::from(y)),
        (text_x + typed % text_width, start + typed / text_width)
    );
}

#[test]
fn test_no_prompt_while_a_command_runs() {
    let mut app = app_with_model(None);
    app.set_shell_prompt("[u@h d]$ ");
    submit_message(&mut app, "$sleep 5");
    let screen = render_rows(&mut app, 120).join("\n");
    assert_eq!(screen.matches("[u@h d]$ ").count(), 1, "{screen}");
    assert!(screen.contains("[u@h d]$ sleep 5"), "{screen}");
    assert!(!screen.contains("running sleep"), "{screen}");
}

#[test]
fn test_terminal_pane_shows_only_commands_and_chat_hides_them() {
    use crate::components::{Message, MessageRole};
    use crate::events::UiCommand;
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = app_with_model(None);
    app.add_message(Message::new(MessageRole::User, "hello agent"));
    submit_message(&mut app, "$echo hi");
    app.process_command(UiCommand::CommandFinished {
        output: "hi\n".to_string(),
        exit_code: None,
    });
    app.process_command(UiCommand::Complete);

    let screen = render_rows(&mut app, 120).join("\n");
    assert!(screen.contains(" TERMINAL "), "{screen}");
    assert!(!screen.contains(" CHAT "), "{screen}");
    assert!(screen.contains("echo hi"), "{screen}");
    assert!(!screen.contains("hello agent"), "{screen}");

    app.handle_key(KeyEvent::from(KeyCode::Esc));
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(screen.contains(" CHAT "), "{screen}");
    assert!(screen.contains("hello agent"), "{screen}");
    assert!(!screen.contains("echo hi"), "{screen}");

    type_text(&mut app, "$");
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(screen.contains("echo hi"), "history kept: {screen}");
}

#[test]
fn test_empty_terminal_pane_says_how_to_leave() {
    let mut app = app_with_model(None);
    type_text(&mut app, "$");
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(screen.contains("Esc returns to chat"), "{screen}");
}

#[test]
fn test_outer_text_is_inset_to_panel_borders() {
    let width = 100;
    let rows = render_with_user(None, width);
    let last_col = |row: &str| row.trim_end().chars().count() - 1;
    let inner_right = usize::from(width) - 2;

    assert!(rows[1].starts_with(" AURA"), "{:?}", rows[1]);
    assert_eq!(last_col(&rows[1]), inner_right, "{:?}", rows[1]);
    let input_row = &rows[rows.len() - 2];
    assert_eq!(last_col(input_row), inner_right, "{input_row:?}");
}

#[test]
fn test_copy_notice_is_centered_on_the_aura_cli_line() {
    let mut app = app_with_model(None);
    app.notify_copied(4, &Ok(()));
    let rows = render_rows(&mut app, 120);

    assert!(
        !rows[0].contains("Copied"),
        "notice belongs on the AURA CLI row: {:?}",
        rows[0]
    );
    let row = &rows[1];
    assert!(row.starts_with(" AURA CLI "), "{row:?}");
    let notice = "✓ Copied 4 characters";
    let start = row[..row.find(notice).expect(row)].chars().count();
    let width = notice.chars().count();
    assert_eq!(start, (120 - width) / 2, "{row:?}");
    assert!(
        row.trim_end().ends_with("● local  http://127.0.0.1:8080"),
        "{row:?}"
    );
}

#[test]
fn test_copy_notice_is_skipped_when_it_would_cover_the_header() {
    let mut app = app_with_model(None);
    app.notify_copied(4, &Ok(()));
    let rows = render_rows(&mut app, 60);

    let row = &rows[1];
    assert!(!row.contains("Copied"), "{row:?}");
    assert!(row.starts_with(" AURA CLI "), "{row:?}");
    assert!(
        row.trim_end().ends_with("● local  http://127.0.0.1:8080"),
        "{row:?}"
    );
}

#[test]
fn test_header_shows_logged_in_user() {
    let rows = render_with_user(Some("alice"), 120);
    assert!(rows[1]
        .trim_end()
        .ends_with("● alice (zOS)  http://127.0.0.1:8080"));
    assert!(!rows[..3].join("\n").contains("local"));
}

#[test]
fn test_header_shows_linked_plans_without_zos() {
    let rows = render_rows(&mut app_with_auth(None, &["Claude"]), 120);
    assert!(
        rows[1]
            .trim_end()
            .ends_with("● Claude  http://127.0.0.1:8080"),
        "{:?}",
        rows[1]
    );
    assert!(!rows[..3].join("\n").contains("local"));
}

#[test]
fn test_header_shows_zos_user_and_plans() {
    let rows = render_rows(
        &mut app_with_auth(Some("alice"), &["ChatGPT", "Claude"]),
        120,
    );
    assert!(
        rows[1]
            .trim_end()
            .ends_with("● alice (zOS) · ChatGPT · Claude  http://127.0.0.1:8080"),
        "{:?}",
        rows[1]
    );
}

#[test]
fn test_input_hides_model_when_narrow() {
    let rows = render_with_user(None, 46);
    let input_row = &rows[rows.len() - 2];
    assert!(!input_row.contains("qwen3:4b"));
    assert!(input_row.contains("Ready"));
}

// ========================================================================
// Record Info Truncation Tests
// ========================================================================

#[test]
fn test_truncate_record_info_fits() {
    assert_eq!(
        truncate_record_info("read_file src/main.rs", 40),
        "read_file src/main.rs"
    );
}

#[test]
fn test_truncate_record_info_adds_ellipsis() {
    let out = truncate_record_info("fix the login redirect bug", 10);
    assert_eq!(out, "fix the l…");
    assert!(display_width(&out) <= 10);
}

#[test]
fn test_truncate_record_info_folds_newlines() {
    let out = truncate_record_info("line one\nline two\r\n\tthree", 80);
    assert_eq!(out, "line one line two three");
}

#[test]
fn test_truncate_record_info_wide_chars_no_panic() {
    let out = truncate_record_info("修复登录错误 🚀 then more text", 7);
    assert!(display_width(&out) <= 7, "too wide: {out:?}");
    assert!(out.ends_with('…'));
}

#[test]
fn test_format_elapsed() {
    use super::panels::format_elapsed;
    use std::time::Duration;

    assert_eq!(format_elapsed(Duration::from_secs(7)), "7s");
    assert_eq!(format_elapsed(Duration::from_secs(125)), "2m 05s");
}

fn submit_message(app: &mut crate::App, text: &str) {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    for c in text.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
}

#[test]
fn test_chat_shows_working_indicator_until_reply_streams() {
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    submit_message(&mut app, "hi");
    let chat = render_rows(&mut app, 120).join("\n");
    assert!(chat.contains("AURA  "), "{chat}");
    assert!(chat.contains("thinking…"), "{chat}");

    app.process_command(UiCommand::StartStreaming);
    assert!(render_rows(&mut app, 120).join("\n").contains("thinking…"));

    app.process_command(UiCommand::AppendText("hello".to_string()));
    assert!(!render_rows(&mut app, 120).join("\n").contains("thinking…"));

    app.process_command(UiCommand::Complete);
    assert!(app.working_label().is_none());
}

/// The chat row of the reply header, after a turn whose reply came from
/// `model` with `usage`, drawn `width` columns wide.
fn reply_header_row(model: &str, usage: Option<CallUsage>, width: u16) -> String {
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    submit_message(&mut app, "hi");
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("hello".to_string()));
    app.process_command(UiCommand::SetReplyModel {
        model: model.to_string(),
        usage,
    });
    app.process_command(UiCommand::Complete);

    let rows = render_rows(&mut app, width);
    let you = rows.iter().find(|r| r.contains("YOU  ")).unwrap();
    assert!(!you.contains(" · "), "{you:?}");
    let aura = rows.iter().find(|r| r.contains("AURA  ")).unwrap();
    let time = app.messages().back().unwrap().timestamp_local();
    assert!(aura.contains(&format!("AURA  {time}")), "{aura:?}");
    let header = &aura[aura.find("AURA  ").unwrap()..];
    header[..header.find('│').unwrap()].to_string()
}

#[test]
fn test_reply_header_shows_model_tokens_and_cost_at_the_right() {
    let usage = CallUsage {
        tokens: 12_400,
        cost_usd: Some(0.0418),
    };
    let header = reply_header_row("claude-opus-4-7", Some(usage), 200);
    let meta = "claude-opus-4-7 · 12.4k tok · $0.042";
    assert!(header.trim_end().ends_with(meta), "{header:?}");
    assert!(
        header.contains(&format!("    {meta}")),
        "meta should sit apart from the time: {header:?}"
    );
}

#[test]
fn test_reply_header_without_usage_or_cost() {
    let header = reply_header_row("qwen3:4b", None, 200);
    assert!(header.trim_end().ends_with("qwen3:4b"), "{header:?}");

    let local = CallUsage {
        tokens: 900,
        cost_usd: None,
    };
    let header = reply_header_row("qwen3:4b", Some(local), 200);
    assert!(
        header.trim_end().ends_with("qwen3:4b · 900 tok"),
        "{header:?}"
    );
}

#[test]
fn test_reply_header_drops_cost_then_tokens_when_narrow() {
    use super::segments::{message_header, reply_meta};

    let usage = CallUsage {
        tokens: 1_250_000,
        cost_usd: Some(3.5),
    };
    let meta = reply_meta(Some("claude-opus-4-7"), Some(usage));
    let header = |width| {
        let line = message_header(
            "AURA",
            ratatui::style::Color::Reset,
            "12:00:00",
            &meta,
            width,
            &test_theme(),
        );
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    };
    assert_eq!(
        header(50),
        "AURA  12:00:00  claude-opus-4-7 · 1.2M tok · $3.50"
    );
    assert!(header(45).ends_with(" claude-opus-4-7 · 1.2M tok"));
    assert!(header(35).ends_with(" claude-opus-4-7"));
    assert!(header(25).ends_with(" claude-o…"));
    for width in [45, 35, 25] {
        assert_eq!(display_width(&header(width)), width, "{:?}", header(width));
    }
    assert_eq!(header(17), "AURA  12:00:00");
}

#[test]
fn test_reply_meta_formats_tokens_and_cost() {
    use super::segments::reply_meta;

    let meta =
        |tokens, cost_usd| reply_meta(Some("m"), Some(CallUsage { tokens, cost_usd })).join(" · ");
    assert_eq!(meta(842, Some(0.004)), "m · 842 tok · <$0.01");
    assert_eq!(meta(12_449, Some(0.0418)), "m · 12.4k tok · $0.042");
    assert_eq!(meta(240_000, Some(1.234)), "m · 240k tok · $1.23");
    assert_eq!(meta(3_400_000, None), "m · 3.4M tok");
    assert_eq!(reply_meta(None, None), Vec::<String>::new());
}

#[test]
fn test_working_indicator_names_running_tool() {
    use crate::events::{ToolData, UiCommand};

    let mut app = crate::App::new();
    submit_message(&mut app, "read it");
    app.process_command(UiCommand::ShowTool(ToolData {
        id: "t1".to_string(),
        name: "read_file".to_string(),
        args: String::new(),
    }));
    assert_eq!(app.working_label().as_deref(), Some("running read_file"));

    app.process_command(UiCommand::CompleteTool {
        id: "t1".to_string(),
        result: "no such file".to_string(),
        success: false,
    });
    assert_eq!(app.working_label().as_deref(), Some("thinking"));
}

#[test]
fn test_turn_activity_renders_inline_between_prompt_and_reply() {
    use crate::events::{ToolData, UiCommand};

    let mut app = crate::App::new();
    submit_message(&mut app, "fix it");
    app.process_command(UiCommand::StartThinking);
    app.process_command(UiCommand::AppendThinking("Look at main first.".to_string()));
    let live = render_rows(&mut app, 120).join("\n");
    assert!(live.contains("Thinking…"), "{live}");
    assert!(live.contains("│ Look at main first."), "{live}");
    assert!(!live.contains("Thinker"), "{live}");

    app.process_command(UiCommand::FinishThinking);
    for (id, path) in [("t1", "src/main.rs"), ("t2", "src/lib.rs")] {
        app.process_command(UiCommand::ShowTool(ToolData {
            id: id.to_string(),
            name: "read_file".to_string(),
            args: format!(r#"{{"path":"{path}"}}"#),
        }));
        app.process_command(UiCommand::CompleteTool {
            id: id.to_string(),
            result: "fn main() {}".to_string(),
            success: true,
        });
    }
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("Fixed.".to_string()));
    app.process_command(UiCommand::Complete);

    let rows = render_rows(&mut app, 120);
    let find = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not in:\n{}", rows.join("\n")))
    };
    let summary = find("▸ Explored 2 files");
    let reply = find("Fixed.");
    assert!(find("fix it") < summary);
    assert!(summary < reply);
    let text = rows.join("\n");
    assert!(!text.contains("Read main.rs"), "{text}");
    assert!(!text.contains("Look at main first."), "{text}");

    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('o'),
        crossterm::event::KeyModifiers::CONTROL,
    ));
    let rows = render_rows(&mut app, 120);
    let text = rows.join("\n");
    assert!(text.contains("▾ Explored 2 files"), "{text}");
    assert!(text.contains("Thought briefly"), "{text}");
    assert!(text.contains("│ Look at main first."), "{text}");
    assert!(text.contains("Read main.rs"), "{text}");
    assert!(text.contains("Read lib.rs"), "{text}");
}

#[test]
fn test_step_group_runs_open_then_collapses_and_clicks_open() {
    use crate::events::{ToolData, UiCommand};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let mut app = crate::App::new();
    submit_message(&mut app, "look around");
    for (id, path) in [("t1", "src/main.rs"), ("t2", "src/lib.rs")] {
        app.process_command(UiCommand::ShowTool(ToolData {
            id: id.to_string(),
            name: "read_file".to_string(),
            args: format!(r#"{{"path":"{path}"}}"#),
        }));
        app.process_command(UiCommand::CompleteTool {
            id: id.to_string(),
            result: "fn main() {}".to_string(),
            success: true,
        });
    }
    let live = render_rows(&mut app, 120).join("\n");
    assert!(live.contains("Exploring 2 files"), "{live}");
    assert!(live.contains("Read main.rs"), "{live}");
    assert!(live.contains("Read lib.rs"), "{live}");
    assert!(!live.contains("thinking…"), "{live}");

    app.process_command(UiCommand::Complete);
    let done = render_rows(&mut app, 120).join("\n");
    assert!(done.contains("▸ Explored 2 files"), "{done}");
    assert!(!done.contains("Read main.rs"), "{done}");

    let click = |app: &mut crate::App, needle: &str| {
        let buffer = render_buffer(app, 120);
        let row = row_of(&buffer, needle);
        let (column, _) = find_cell(&buffer, row, needle).unwrap();
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.handle_mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::empty(),
            });
        }
    };
    click(&mut app, "Explored");
    let open = render_rows(&mut app, 120).join("\n");
    assert!(open.contains("▾ Explored 2 files"), "{open}");
    assert!(open.contains("Read lib.rs"), "{open}");
    assert!(!open.contains("│ fn main() {}"), "{open}");

    click(&mut app, "lib.rs");
    let step = render_rows(&mut app, 120).join("\n");
    assert!(step.contains("▾ Read lib.rs"), "{step}");
    assert!(step.contains("│ fn main() {}"), "{step}");
}

#[test]
fn test_policy_decisions_fold_into_their_tool_calls() {
    use crate::events::{ToolData, UiCommand};

    let mut app = crate::App::new();
    submit_message(&mut app, "kill it then respawn");
    for id in ["t1", "t2", "t3"] {
        app.process_command(UiCommand::ShowTool(ToolData {
            id: id.to_string(),
            name: "run_command".to_string(),
            args: r#"{"command":"node -e 1"}"#.to_string(),
        }));
        app.process_command(UiCommand::ToolPolicy {
            id: id.to_string(),
            tool: "run_command".to_string(),
            decision: "allowed by user_default.full_access".to_string(),
        });
        app.process_command(UiCommand::CompleteTool {
            id: id.to_string(),
            result: "ok".to_string(),
            success: id != "t1",
        });
    }
    app.process_command(UiCommand::Complete);

    let done = render_rows(&mut app, 120).join("\n");
    assert!(done.contains("▸ Ran 3 commands · 1 failed"), "{done}");
    assert!(!done.contains("policy:"), "{done}");

    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('o'),
        crossterm::event::KeyModifiers::CONTROL,
    ));
    let open = render_rows(&mut app, 120).join("\n");
    assert!(open.contains("▾ Ran node -e 1"), "{open}");
    assert!(open.contains("│ policy: allowed by"), "{open}");
    assert!(!open.contains("policy: run_command"), "{open}");

    app.process_command(UiCommand::ToolPolicy {
        id: "gone".to_string(),
        tool: "write_file".to_string(),
        decision: "denied by user.off".to_string(),
    });
    let rows = render_rows(&mut app, 120).join("\n");
    assert!(rows.contains("policy: write_file denied by user.off"), "{rows}");
}

#[test]
fn test_policy_lines_share_a_row_with_their_time_and_stack() {
    use crate::events::{MessageData, MessageRole, UiCommand};

    let mut app = crate::App::new();
    submit_message(&mut app, "write them");
    for tool in ["write_file", "edit_file"] {
        app.process_command(UiCommand::ShowMessage(MessageData::new(
            MessageRole::System,
            format!("policy: {tool} allowed by user.on"),
        )));
    }
    let rows = render_rows(&mut app, 120);
    let row = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not in:\n{}", rows.join("\n")))
    };
    let first = row("policy: write_file allowed by user.on");
    let time = app
        .messages()
        .iter()
        .rev()
        .nth(1)
        .unwrap()
        .timestamp_local();
    assert!(
        rows[first].contains(&format!("*  {time}  policy: write_file")),
        "{}",
        rows[first]
    );
    assert_eq!(row("policy: edit_file allowed by user.on"), first + 1);
}

#[test]
fn test_truncate_record_info_zero_width() {
    assert_eq!(truncate_record_info("anything", 0), "");
    assert_eq!(truncate_record_info("", 20), "");
}

fn render_buffer(app: &mut crate::App, width: u16) -> ratatui::buffer::Buffer {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30)).unwrap();
    terminal
        .draw(|frame| super::render(frame, app, &test_theme()))
        .unwrap();
    terminal.backend().buffer().clone()
}

/// First row that contains `needle`.
fn row_of(buffer: &ratatui::buffer::Buffer, needle: &str) -> u16 {
    (0..buffer.area.height)
        .find(|&y| find_cell(buffer, y, needle).is_some())
        .unwrap_or_else(|| panic!("{needle:?} not on screen"))
}

/// Column of `needle` in row `y`, and the cell style there.
fn find_cell(buffer: &ratatui::buffer::Buffer, y: u16, needle: &str) -> Option<(u16, Style)> {
    let needle: Vec<String> = needle.chars().map(String::from).collect();
    let width = buffer.area.width;
    let n = u16::try_from(needle.len()).ok()?;
    let col = (0..width.saturating_sub(n)).find(|&x| {
        needle
            .iter()
            .zip(x..)
            .all(|(c, cx)| buffer[(cx, y)].symbol() == c)
    })?;
    Some((col, buffer[(col, y)].style()))
}

#[test]
fn test_scrolled_chat_stays_put_while_reply_streams() {
    use crate::components::{Message, MessageRole};
    use crate::events::UiCommand;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = crate::App::new();
    for i in 0..40 {
        app.add_message(Message::new(
            MessageRole::Assistant,
            &format!("note {i:02}"),
        ));
    }
    render_rows(&mut app, 100);
    app.scroll_up(12);
    let notes = |rows: Vec<String>| -> Vec<String> {
        rows.into_iter()
            .filter(|row| row.contains("note "))
            .collect()
    };
    let before = render_rows(&mut app, 100);
    assert!(before.join("\n").contains("↓ 12 lines below"));

    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("first\nsecond\nthird".to_string()));
    let after = render_rows(&mut app, 100);
    let screen = after.join("\n");
    assert!(!screen.contains("third"), "{screen}");
    assert!(screen.contains("lines below"), "{screen}");
    assert_eq!(notes(after), notes(before));

    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL));
    let screen = render_rows(&mut app, 100).join("\n");
    assert!(screen.contains("third"), "{screen}");
    assert!(!screen.contains("lines below"), "{screen}");
}

#[test]
fn test_cached_chat_lines_follow_message_edits() {
    use crate::components::{Message, MessageRole};
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("draft".to_string()));
    render_rows(&mut app, 100);
    app.process_command(UiCommand::AppendText(" reply".to_string()));
    app.add_message(Message::new(MessageRole::User, "next"));
    app.scroll_chat_to_top();
    let screen = render_rows(&mut app, 100).join("\n");
    assert!(screen.contains("draft reply"), "{screen}");
    assert!(screen.contains("next"), "{screen}");

    let narrow = render_rows(&mut app, 60).join("\n");
    assert!(narrow.contains("draft reply"), "{narrow}");
}

#[test]
fn test_chat_bottom_leaves_room_below_latest_prompt() {
    use crate::components::{Message, MessageRole};

    let mut app = crate::App::new();
    for i in 0..40 {
        app.add_message(Message::new(
            MessageRole::Assistant,
            &format!("note {i:02}"),
        ));
    }
    app.add_message(Message::new(MessageRole::User, "latest prompt"));
    app.add_message(Message::new(MessageRole::Assistant, "short reply"));
    let rows = render_rows(&mut app, 100);
    let screen = rows.join("\n");
    let row = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not in:\n{screen}"))
    };
    assert!(row("latest prompt") <= row(" CHAT ") + 3, "{screen}");
    assert!(screen.contains("short reply"), "{screen}");
    assert!(!screen.contains("note 39"), "{screen}");
    assert!(!screen.contains("lines below"), "{screen}");

    app.scroll_up(3);
    let screen = render_rows(&mut app, 100).join("\n");
    assert!(!screen.contains("lines below"), "{screen}");
}

#[test]
fn test_command_input_is_highlighted() {
    let theme = test_theme();
    let mut app = app_with_model(None);
    type_text(&mut app, "$echo -n hi");
    let buffer = render_buffer(&mut app, 120);
    let input_y = row_of(&buffer, "]$ echo -n hi");

    let (_, program) = find_cell(&buffer, input_y, "echo").unwrap();
    assert_eq!(program.fg, Some(theme.colors.success));
    assert!(program.add_modifier.contains(Modifier::BOLD));
    let (_, flag) = find_cell(&buffer, input_y, "-n").unwrap();
    assert_eq!(flag.fg, Some(theme.colors.warning));
}

#[test]
fn test_prose_input_stays_muted() {
    let theme = test_theme();
    let mut app = app_with_model(None);
    type_text(&mut app, "please explain");
    let buffer = render_buffer(&mut app, 120);

    let (_, style) = find_cell(&buffer, buffer.area.height - 2, "please").unwrap();
    assert_eq!(style.fg, Some(theme.colors.muted));
}

#[test]
fn test_command_output_renders_like_a_terminal() {
    use crate::events::UiCommand;

    let theme = test_theme();
    let mut app = app_with_model(None);
    app.set_shell_prompt("[u@h d]$ ");
    submit_message(&mut app, "$ls -la");
    app.process_command(UiCommand::CommandFinished {
        output: "total 0\ndrwxr-xr-x  2 u u  40 \x1b[01;34msrc\x1b[0m\n".to_string(),
        exit_code: None,
    });
    app.process_command(UiCommand::Complete);

    let buffer = render_buffer(&mut app, 120);
    let rows = render_rows(&mut app, 120);
    let screen = rows.join("\n");
    let echo = rows
        .iter()
        .position(|r| r.contains("[u@h d]$ ls -la"))
        .unwrap();
    assert!(!screen.contains("YOU  "), "{screen}");
    let total = &rows[echo + 1];
    let listing = &rows[echo + 2];
    assert!(total.contains("total 0"), "{total:?}");
    assert!(listing.contains("drwxr-xr-x  2 u u  40 src"), "{listing:?}");
    assert!(rows[echo + 3].contains("[u@h d]$ "), "{:?}", rows[echo + 3]);
    assert!(!screen.contains('\x1b'));
    let column = |row: &str, needle: &str| row.find(needle).map(|i| row[..i].chars().count());
    assert_eq!(
        column(total, "total"),
        column(&rows[echo], "[u@h d]"),
        "output starts at the prompt's column"
    );

    let y = u16::try_from(echo + 2).unwrap();
    let (_, dir) = find_cell(&buffer, y, "src").unwrap();
    assert_eq!(dir.fg, Some(Color::Indexed(4)));
    assert!(dir.add_modifier.contains(Modifier::BOLD));
    let (_, plain) = find_cell(&buffer, y, "drwx").unwrap();
    assert_eq!(plain.fg, Some(theme.colors.foreground));
}

#[test]
fn test_failed_command_shows_exit_code() {
    use crate::events::UiCommand;

    let mut app = app_with_model(None);
    submit_message(&mut app, "$false");
    app.process_command(UiCommand::CommandFinished {
        output: String::new(),
        exit_code: Some(1),
    });

    let rows = render_rows(&mut app, 120);
    let echo = rows.iter().position(|r| r.contains("]$ false")).unwrap();
    assert!(rows[echo + 1].contains("exit 1"), "{:?}", rows[echo + 1]);
}

#[test]
fn test_chat_opens_at_last_message_on_startup() {
    use crate::components::MessageRole;
    use crate::events::{MessageData, UiCommand};

    let history = |tag: &str| {
        (0..40)
            .map(|i| MessageData::new(MessageRole::User, format!("{tag} {i}")))
            .collect()
    };
    let shows = |rows: &[String], text: &str| rows.iter().any(|r| r.contains(&format!("{text} ")));

    let (tx, rx) = tokio::sync::mpsc::channel(200);
    let mut app = crate::App::new().with_command_receiver(rx);
    tx.try_send(UiCommand::LoadConversation(history("first")))
        .unwrap();
    app.tick();
    let rows = render_rows(&mut app, 120);
    assert!(shows(&rows, "first 39"), "{}", rows.join("\n"));
    assert!(!shows(&rows, "first 0"), "{}", rows.join("\n"));

    // Opening another chat after scrolling this one up starts at its end too.
    app.scroll_up(30);
    render_rows(&mut app, 120);
    tx.try_send(UiCommand::LoadConversation(history("second")))
        .unwrap();
    app.tick();
    let rows = render_rows(&mut app, 120);
    assert!(shows(&rows, "second 39"), "{}", rows.join("\n"));
}

/// The right border of the panel titled `title`, top to bottom, without
/// its corners.
fn right_border(rows: &[String], title: &str) -> String {
    let top = rows.iter().position(|r| r.contains(title)).unwrap();
    let start = rows[top][..rows[top].find(title).unwrap()].chars().count();
    let column = start
        + rows[top]
            .chars()
            .skip(start)
            .position(|c| c == '┐')
            .unwrap();
    rows[top + 1..]
        .iter()
        .map(|r| r.chars().nth(column).unwrap())
        .take_while(|&c| c != '┘')
        .collect()
}

fn chat_track(rows: &[String]) -> String {
    right_border(rows, "┌ CHAT").chars().skip(1).collect()
}

#[test]
fn test_record_rows_align_kind_what_and_group_columns() {
    use crate::events::{RecordStatus, RecordSummary, UiCommand};

    let mut app = crate::App::new();
    for (seq, kind, what, group, detail) in [
        (1, "Policy", "read_file", "file", "allow · user_default"),
        (2, "ToolCall", "run_command", "shell", "cargo test"),
    ] {
        app.process_command(UiCommand::NewRecord(RecordSummary {
            seq,
            timestamp: "12:00:00".to_string(),
            full_hash: String::new(),
            hash_suffix: String::new(),
            tx_kind: kind.to_string(),
            sender: String::new(),
            message: String::new(),
            action_count: 0,
            effect_status: String::new(),
            status: RecordStatus::Ok,
            info: String::new(),
            what: what.to_string(),
            group: group.to_string(),
            detail: detail.to_string(),
            error_details: String::new(),
            tx_id: String::new(),
            agent_id: String::new(),
            ts_ms: 0,
            policy: None,
        }));
    }
    let rows = render_rows(&mut app, 200);
    let screen = rows.join("\n");
    let row = |needle: &str| {
        rows.iter()
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not in:\n{screen}"))
    };
    let column = |row: &str, needle: &str| row[..row.find(needle).unwrap()].chars().count();
    let policy = row("READ_FILE");
    let call = row("RUN_COMMAND");

    assert_eq!(column(policy, "POLICY"), column(call, "TOOLCALL"));
    assert_eq!(column(policy, "READ_FILE"), column(call, "RUN_COMMAND"));
    assert_eq!(column(policy, " FILE "), column(call, " SHELL "));
    assert_eq!(column(policy, "allow"), column(call, "cargo test"));
    assert!(!screen.contains("12:00:00"), "{screen}");
}

#[test]
fn test_overflowing_panels_show_a_scrollbar() {
    use crate::components::{Message, MessageRole};
    use crate::events::{RecordStatus, RecordSummary, UiCommand};

    let mut app = crate::App::new();
    app.add_message(Message::new(MessageRole::User, "short"));
    let rows = render_rows(&mut app, 120);
    assert!(!rows.join("\n").contains('┃'), "{}", rows.join("\n"));

    for i in 0..40 {
        app.add_message(Message::new(MessageRole::User, &format!("message {i}")));
        app.process_command(UiCommand::NewRecord(RecordSummary {
            seq: i,
            timestamp: "12:00:00".to_string(),
            full_hash: String::new(),
            hash_suffix: String::new(),
            tx_kind: "Prompt".to_string(),
            sender: String::new(),
            message: String::new(),
            action_count: 0,
            effect_status: String::new(),
            status: RecordStatus::Ok,
            info: format!("record {i}"),
            what: String::new(),
            group: String::new(),
            detail: String::new(),
            error_details: String::new(),
            tx_id: String::new(),
            agent_id: String::new(),
            ts_ms: 0,
            policy: None,
        }));
    }
    let rows = render_rows(&mut app, 120);
    let screen = rows.join("\n");

    // The chat sits at its end, so the thumb is at the bottom of the track,
    // which starts under the chat's blank top row.
    let chat = chat_track(&rows);
    assert!(
        chat.starts_with('│') && chat.ends_with('┃'),
        "{chat:?}\n{screen}"
    );
    // Records list oldest first and follow the newest at the bottom.
    let records = right_border(&rows, "┌ KERNEL");
    assert!(
        records.starts_with('│') && records.ends_with('┃'),
        "{records:?}\n{screen}"
    );

    app.scroll_chat_to_top();
    let rows = render_rows(&mut app, 120);
    let chat = chat_track(&rows);
    assert!(chat.starts_with('┃') && chat.ends_with('│'), "{chat:?}");
}
