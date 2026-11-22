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

    assert_eq!(segments.len(), 2);
    assert!(matches!(&segments[0], ContentSegment::Text(text) if text == "Before"));
    assert!(matches!(
        &segments[1],
        ContentSegment::CodeBlock { language, code } if language == "rust" && code == "fn main() {}"
    ));
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
            ..Default::default()
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
    // A project row shows its first agent's name in capitals.
    let [terminal_project] = rows_with("TERMINAL AGENT")[..] else {
        panic!("expected one project row for Terminal Agent");
    };
    let [terminal_agent] = rows_with("Terminal Agent")[..] else {
        panic!("expected one agent row for Terminal Agent");
    };
    let [researcher_project] = rows_with("RESEARCHER")[..] else {
        panic!("expected one project row for Researcher");
    };
    let [researcher_agent] = rows_with("Researcher")[..] else {
        panic!("expected one agent row for Researcher");
    };
    let new_project = rows_with("New project")[0];
    assert!(rows[terminal_project].contains("🗁  TERMINAL AGENT"));
    // An idle agent leaves the icon column blank, even when it is active.
    let agent_panel: String = rows[terminal_agent].chars().take(30).collect();
    assert!(
        !agent_panel.contains('●') && !agent_panel.contains('·'),
        "{agent_panel:?}"
    );
    assert_eq!(
        char_col(&rows[terminal_agent], "Terminal Agent"),
        char_col(&rows[terminal_project], "TERMINAL AGENT")
    );
    assert!(rows[terminal_agent].contains(" 3m "));
    assert!(rows[researcher_project].contains("🗁  RESEARCHER"));
    assert!(!screen.contains("12│"));
    let above_footer = rows[..rows.len() - 1].join("\n");
    assert!(!above_footer.contains("# ") && !above_footer.contains("@ "));
    assert!(!screen.contains('↳') && !screen.contains('▸'));
    // Only Researcher has a spawn, and it is thinking: its ring stands in
    // for the open marker.
    assert!(rows_with("▾").is_empty());
    assert!(!rows[researcher_agent].contains('▾'));

    // A thinking agent shows the chat's braille spinner in the icon column.
    let ring_col = char_col(&rows[terminal_project], "🗁");
    let ring: String = rows[researcher_agent]
        .chars()
        .skip(ring_col)
        .take(crate::animation::AURA_WIDTH)
        .collect();
    assert!(
        is_aura_ring(&ring),
        "expected the braille spinner, got {ring:?}"
    );

    // Icons and names line up down the list, with a blank row between projects.
    assert_eq!(
        char_col(&rows[terminal_project], "🗁"),
        char_col(&rows[new_project], "+")
    );
    assert_eq!(ring_col, char_col(&rows[terminal_project], "🗁"));
    assert_eq!(
        char_col(&rows[new_project], "New project"),
        char_col(&rows[researcher_project], "RESEARCHER")
    );
    // Projects and agents are one line each, with one blank line between projects.
    assert_eq!(terminal_agent, terminal_project + 1);
    assert_eq!(researcher_project, terminal_agent + 2);
    let panel_text = |row: usize| rows[row].chars().skip(2).take(20).collect::<String>();
    assert_eq!(panel_text(terminal_agent + 1).trim(), "");
    assert_eq!(researcher_agent, researcher_project + 1);
    // "+ New project" takes the panel's bottom row in place of a key hint.
    assert!(!screen.contains("space fold") && !screen.contains("^N new"));
    assert!(
        rows[new_project + 1].starts_with(" └"),
        "New project should sit on the panel's last row: {:?}",
        &rows[new_project..=new_project + 1]
    );
    assert!(new_project > rows_with("reviewer #1")[0] + 1);
    // Spawned agents sit further in than the project's first agent.
    assert!(
        char_col(&rows[rows_with("reviewer #1")[0]], "reviewer #1")
            > char_col(&rows[researcher_agent], "Researcher")
    );
    // The spawn's tree line drops from under its parent's name and runs
    // through the empty icon column into the spawn's name.
    let reviewer = &rows[rows_with("reviewer #1")[0]];
    let elbow = char_col(reviewer, "└──");
    assert_eq!(elbow, char_col(&rows[researcher_agent], "Researcher"));
    assert_eq!(char_col(reviewer, "reviewer #1"), elbow + 4, "{reviewer:?}");
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
    // The roster `@` is gray; the agent name stays white. Selection paints
    // both in the selection text color, so step off the row first.
    app.handle_key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Down,
    ));
    let buffer = render_buffer(&mut app, 120);
    let theme = test_theme();
    let reviewer_row = row_of(&buffer, "reviewer");
    let (_, at) = find_cell(&buffer, reviewer_row, "@").unwrap();
    let (_, name) = find_cell(&buffer, reviewer_row, "reviewer").unwrap();
    assert_eq!(at.fg, Some(theme.colors.muted));
    assert_eq!(name.fg, Some(theme.colors.secondary));
    app.handle_key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Up,
    ));
    assert!(!screen.contains("spawn") && !screen.contains(" del"));
    assert!(!screen.contains("Researcher"));
    // "+ New agent" takes the panel's bottom row, like "+ New project".
    let new_agent = rows.iter().position(|r| r.contains("New agent")).unwrap();
    assert!(
        rows[new_agent + 1].starts_with(" └"),
        "New agent should sit on the panel's last row: {:?}",
        &rows[new_agent..=new_agent + 1]
    );
    assert!(new_agent > rows.iter().position(|r| r.contains("@  reviewer")).unwrap() + 1);
}

/// An app showing the roster of `definitions`, each a name and its
/// abbreviation.
fn roster_app(definitions: &[(&str, Option<&str>)]) -> crate::App {
    use crate::app::{AgentsTab, PaneTab};
    use crate::events::{AgentDefinitionSummary, UiCommand};
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgentDefinitions(
        definitions
            .iter()
            .map(|&(name, abbreviation)| AgentDefinitionSummary {
                name: name.to_string(),
                description: format!("What {name} does"),
                abbreviation: abbreviation.map(str::to_string),
            })
            .collect(),
    ));
    app.select_pane_tab(PaneTab::Agents(AgentsTab::Roster));
    app
}

/// The text of each of `lines`.
fn line_texts(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

#[test]
fn roster_rows_end_in_their_abbreviation_lined_up_at_the_far_right() {
    let mut app = roster_app(&[
        ("rust-engineer", Some("RE")),
        ("reviewer", Some("REV")),
        ("explorer", Some("EX")),
    ]);
    let rows = render_rows(&mut app, 120);
    // The panel's text from just after the name to its right border.
    let end_of = |name: &str| -> String {
        let row = &rows[line_with(&rows, &format!("@  {name}"))];
        let after = &row[row.find(name).unwrap() + name.len()..];
        after[..after.find('│').expect("border")].to_string()
    };
    for (name, abbreviation) in [
        ("rust-engineer", "RE "),
        ("reviewer", "REV"),
        ("explorer", "EX "),
    ] {
        let end = end_of(name);
        assert!(
            end.ends_with(&format!(" {abbreviation} ")),
            "{name}: {end:?}"
        );
        assert!(
            end.trim_start().starts_with(abbreviation.trim_end()),
            "{end:?}"
        );
    }
    let column = |name: &str| {
        let row = &rows[line_with(&rows, &format!("@  {name}"))];
        char_column(row, name) + name.len() + end_of(name).len() - 4
    };
    assert_eq!(column("rust-engineer"), column("reviewer"));
    assert_eq!(column("reviewer"), column("explorer"));

    // "+ New agent" has none.
    let new_agent = &rows[line_with(&rows, "+  New agent")];
    let after = &new_agent[new_agent.find("New agent").unwrap() + "New agent".len()..];
    let after = &after[..after.find('│').expect("border")];
    assert!(after.chars().all(|c| c == ' '), "{new_agent:?}");

    // The abbreviation is in the time color on the row's fill, on the
    // selected first row as on the others.
    let theme = test_theme();
    let buffer = render_buffer(&mut app, 120);
    for (name, abbreviation) in [("rust-engineer", "RE"), ("explorer", "EX")] {
        let line = row_of(&buffer, name);
        let (_, cell) = find_cell(&buffer, line, &format!("{abbreviation} ")).unwrap();
        let (_, at) = find_cell(&buffer, line, "@").unwrap();
        assert_eq!(cell.bg, at.bg, "{name}");
        let time = if name == "rust-engineer" {
            theme.colors.on_selection
        } else {
            theme.colors.muted
        };
        assert_eq!(cell.fg, Some(time), "{name}");
    }
}

#[test]
fn a_narrow_roster_cuts_the_name_then_drops_the_abbreviation() {
    use crate::animation::AURA_WIDTH;
    let app = roster_app(&[("rust-engineer", Some("RE")), ("reviewer", Some("REV"))]);
    let theme = test_theme();
    // ` `, the icon column and its pad, then ` REV `.
    let around = 1 + AURA_WIDTH + 1 + 5;
    for width in around..=around + 20 {
        let inner = Rect::new(0, 0, u16::try_from(width).unwrap(), 10);
        let lines = super::panels::build_definition_lines(&app, inner, &theme);
        let texts = line_texts(&lines);
        for (line, text) in lines.iter().zip(&texts) {
            assert_eq!(line.width(), width, "width {width}: {text:?}");
        }
        let [engineer, reviewer, new_agent] = &texts[..] else {
            panic!("{texts:?}");
        };
        if width >= around + 4 {
            assert!(engineer.ends_with(" RE  "), "{engineer:?}");
            assert!(reviewer.ends_with(" REV "), "{reviewer:?}");
            assert!(
                engineer.contains("rust-engineer") || engineer.contains('…'),
                "the name is cut first: {engineer:?}"
            );
        } else {
            assert!(!engineer.contains("RE"), "{engineer:?}");
            assert!(!reviewer.contains("REV"), "{reviewer:?}");
        }
        let lead = " ".repeat(AURA_WIDTH);
        assert!(new_agent.starts_with(&format!(" +{lead}")), "{new_agent:?}");
        assert!(!new_agent.contains("RE"), "{new_agent:?}");
    }

    // Roster rows without an abbreviation keep the plain layout.
    let plain = roster_app(&[("reviewer", None)]);
    let inner = Rect::new(0, 0, 24, 10);
    let texts = line_texts(&super::panels::build_definition_lines(
        &plain, inner, &theme,
    ));
    let lead = " ".repeat(AURA_WIDTH);
    let name_width = 24 - 2 - AURA_WIDTH;
    assert_eq!(texts[0], format!(" @{lead}{:<name_width$}", "reviewer"));
    assert_eq!(texts[1], format!(" +{lead}{:<name_width$}", "New agent"));
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

/// The tab row of the header bar holding `label`, and the widths of the
/// bar's cells between its frame and dividers. Checks that the bar is
/// framed, with each divider joining the frame, and that the panel's body
/// starts right below it.
fn header_bar(rows: &[String], label: &str) -> (usize, Vec<usize>) {
    let grid: Vec<Vec<char>> = rows.iter().map(|r| r.chars().collect()).collect();
    let tabs = rows
        .iter()
        .position(|r| r.contains(label))
        .unwrap_or_else(|| panic!("no {label} in:\n{}", rows.join("\n")));
    let at = rows[tabs][..rows[tabs].find(label).unwrap()]
        .chars()
        .count();
    let left = grid[tabs][..=at].iter().rposition(|&c| c == '│').unwrap();
    let right = left
        + grid[tabs - 1][left..]
            .iter()
            .position(|&c| c == '┐')
            .unwrap();

    assert_eq!(grid[tabs - 1][left], '┌');
    assert_eq!(grid[tabs + 1][left], '└');
    assert_eq!(grid[tabs + 1][right], '┘');
    assert_eq!(grid[tabs + 2][left], '┌', "body right below the bar");
    let mut widths = Vec::new();
    let mut start = left + 1;
    for x in left + 1..=right {
        if grid[tabs][x] == '│' {
            if x < right {
                assert_eq!(grid[tabs - 1][x], '┬', "column {x}");
                assert_eq!(grid[tabs + 1][x], '┴', "column {x}");
            }
            widths.push(x - start);
            start = x + 1;
        }
    }
    (tabs, widths)
}

#[test]
fn test_agents_header_is_a_bar_of_equal_tabs() {
    let mut app = crate::App::new();
    for width in [120, 160] {
        let rows = render_rows(&mut app, width);
        let (tabs, widths) = header_bar(&rows, "PROJECTS");
        assert!(rows[tabs].contains("AGENTS"));
        assert_eq!(widths.len(), 3, "the tabs fill the bar: {widths:?}");
        let (min, max) = (widths.iter().min(), widths.iter().max());
        assert!(max.zip(min).is_some_and(|(a, b)| a - b <= 1), "{widths:?}");
    }
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
    let project_row = row_of(&buffer, "TERMINAL AGENT");
    let selected_row = project_row + 1;
    let gap_row = project_row + 2;
    let researcher_project = project_row + 3;
    let researcher_agent = project_row + 4;
    assert!(find_cell(&buffer, selected_row, "Terminal Agent").is_some());
    assert!(find_cell(&buffer, researcher_project, "RESEARCHER").is_some());
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
    let (_, project) = find_cell(&buffer, researcher_project, "RESEARCHER").unwrap();
    assert_eq!(project.bg, Some(colors.surface_hover));
    assert_eq!(project.fg, Some(colors.muted));
    assert!(!project.add_modifier.contains(Modifier::BOLD));
    let (_, agent) = find_cell(&buffer, researcher_agent, "Researcher").unwrap();
    assert_eq!(agent.bg, Some(Color::Reset));

    // Move off the list so the checks below see no hover.
    hover(&mut app, gap_row);
    let buffer = render_buffer(&mut app, 120);

    // The selected cell runs from one panel border to the other; nothing
    // else has a background.
    let left_border = 1;
    assert_eq!(buffer[(left_border, selected_row)].symbol(), "│");
    let right_border = (left_border + 1..buffer.area.width)
        .find(|&x| buffer[(x, selected_row)].symbol() == "│")
        .unwrap();
    let (left, right) = (left_border + 1, right_border - 1);
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
    let (_, project) = find_cell(&buffer, project_row, "TERMINAL AGENT").unwrap();
    assert_eq!(project.bg, Some(Color::Reset));
    assert_eq!(project.fg, Some(colors.muted));
    assert!((project_row..buffer.area.height)
        .all(|y| buffer[(left, y)].style().bg != Some(colors.selection)));
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::empty()));
    assert_eq!(app.selected_agent(), cursor);
    let buffer = render_buffer(&mut app, 120);
    let (_, agent) = find_cell(&buffer, selected_row, "Terminal Agent").unwrap();
    assert_eq!(agent.bg, Some(colors.selection));
    let (_, project) = find_cell(&buffer, project_row, "TERMINAL AGENT").unwrap();
    assert_eq!(project.bg, Some(Color::Reset));

    // Away from the panel, the agent shown in the chat stays selected
    // wherever the cursor was. The other rows sit on the panel background:
    // projects gray, agents white, none of them bold.
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
    for (row, name, fg) in [
        (project_row, "TERMINAL AGENT", colors.muted),
        (researcher_project, "RESEARCHER", colors.muted),
        (researcher_agent, "Researcher", colors.foreground),
    ] {
        let (_, cell) = find_cell(&buffer, row, name).unwrap();
        assert_eq!(cell.bg, Some(Color::Reset), "{name}");
        assert_eq!(cell.fg, Some(fg), "{name}");
        assert!(!cell.add_modifier.contains(Modifier::BOLD), "{name}");
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

#[test]
fn test_edited_file_block_shows_its_diff() {
    use crate::events::{ToolData, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::ShowTool(ToolData {
        id: "t1".to_string(),
        name: "write_file".to_string(),
        args: r#"{"path":"src/lib.rs","content":"fn a() {}\nfn c() {}\n"}"#.to_string(),
    }));
    app.process_command(UiCommand::ToolDiff {
        id: "t1".to_string(),
        diff: "@@ -1,2 +1,2 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n".to_string(),
    });
    app.process_command(UiCommand::CompleteTool {
        id: "t1".to_string(),
        result: "Wrote 20 bytes to src/lib.rs".to_string(),
        success: true,
    });

    let rows = render_rows(&mut app, 120);
    let text = rows.join("\n");
    assert!(text.contains("Wrote lib.rs · +1 -1"), "{text}");
    assert!(!text.contains("fn c()"), "collapsed blocks hide the diff");

    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('o'),
        crossterm::event::KeyModifiers::CONTROL,
    ));
    let text = render_rows(&mut app, 120).join("\n");
    assert!(text.contains("│ src/lib.rs"), "{text}");
    assert!(text.contains("2 │ - fn b() {}"), "{text}");
    assert!(text.contains("2 │ + fn c() {}"), "{text}");
    assert!(
        !text.contains("\"content\""),
        "the diff replaces the raw args"
    );
}

#[test]
fn test_edit_restored_from_history_diffs_its_arguments() {
    use crate::events::{ActivityData, MessageData, MessageRole, UiCommand};

    let mut app = crate::App::new();
    let mut data = MessageData::new(MessageRole::Assistant, "");
    data.activity = Some(ActivityData::Tool {
        id: "t1".to_string(),
        name: "edit_file".to_string(),
        args: r#"{"path":"a.rs","old_text":"let x = 1;","new_text":"let x = 2;"}"#.to_string(),
        result: Some("Replaced 1 occurrence(s) in a.rs".to_string()),
        is_error: false,
        duration_ms: None,
        policy: None,
    });
    app.process_command(UiCommand::LoadConversation(vec![data]));
    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('o'),
        crossterm::event::KeyModifiers::CONTROL,
    ));

    let text = render_rows(&mut app, 120).join("\n");
    assert!(text.contains("Edited a.rs · +1 -1"), "{text}");
    assert!(text.contains("│ - let x = 1;"), "{text}");
    assert!(text.contains("│ + let x = 2;"), "{text}");
}

#[test]
fn test_diff_message_shows_each_file() {
    use crate::events::{FileDiffData, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::ShowDiff {
        title: "Changes in turn 2 · fix it".to_string(),
        files: vec![
            FileDiffData {
                path: "src/a.rs".to_string(),
                diff: "@@ -3,1 +3,1 @@\n-old\n+new\n".to_string(),
            },
            FileDiffData {
                path: "notes.md".to_string(),
                diff: "@@ -0,0 +1,1 @@\n+hello\n".to_string(),
            },
        ],
    });

    let text = render_rows(&mut app, 120).join("\n");
    assert!(text.contains("Changes in turn 2 · fix it"), "{text}");
    assert!(text.contains("src/a.rs  +1 -1"), "{text}");
    assert!(text.contains("3 │ - old"), "{text}");
    assert!(text.contains("3 │ + new"), "{text}");
    assert!(text.contains("notes.md  +1 -0"), "{text}");
    assert!(text.contains("1 │ + hello"), "{text}");
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
    app.scroll_chat_to_top();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(85, 30)).unwrap();
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
    assert!(!screen.contains("AURA  "), "{screen}");
    assert!(screen.contains("Summary"), "{screen}");
    assert!(!screen.contains("##") && !screen.contains("**"), "{screen}");
    assert!(screen.contains("The build is green and cargo"), "{screen}");
    assert!(screen.contains("• [✓] done"), "{screen}");
    assert!(screen.contains("│ aura-terminal │   231 │"), "{screen}");
    assert!(screen.contains("│ note: see docs"), "{screen}");
    assert!(screen.contains("(https://example.com)"), "{screen}");

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
fn test_chat_prose_rows_leave_out_code_and_tables() {
    use crate::components::{Message, MessageRole};

    let mut app = crate::App::new();
    app.add_message(Message::new(MessageRole::User, "run the tests"));
    app.add_message(Message::new(
        MessageRole::Assistant,
        "Run `cargo test` now.\n\n```sh\ncargo test\n```\n\n\
         | Crate | Tests |\n|---|---:|\n| aura-terminal | 231 |\n\n---\n\nAll green.",
    ));
    app.scroll_chat_to_top();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 34)).unwrap();
    terminal
        .draw(|frame| super::render(frame, &mut app, &test_theme()))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let text = |y: u16, columns: std::ops::Range<u16>| -> String {
        columns.map(|x| buffer[(x, y)].symbol()).collect()
    };
    let prose: Vec<String> = app
        .prose_rows()
        .iter()
        .map(|row| text(row.y, row.x..row.x + row.width))
        .collect();
    assert_eq!(
        prose,
        ["→ run the tests", "Run cargo test now.", "All green."],
        "only flowing text is prose"
    );
    let run = &app.prose_rows()[1];
    assert_eq!(run.code.len(), 1);
    assert_eq!(text(run.y, run.code[0].clone()), "cargo test");
}

#[test]
fn test_measured_prose_packs_into_the_cells_it_covers() {
    use crate::components::{Message, MessageRole};

    #[allow(clippy::cast_precision_loss)]
    fn half(text: &str, _: ratatui::style::Modifier) -> f32 {
        text.chars().count() as f32 / 2.0
    }

    let prompt = "word ".repeat(60);
    let prompt = prompt.trim_end();
    let mut app = crate::App::new();
    app.set_prose_measure(half);
    app.add_message(Message::new(MessageRole::User, prompt));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    let frame = terminal
        .draw(|frame| super::render(frame, &mut app, &test_theme()))
        .unwrap();

    let rows = app.prose_rows();
    let texts: Vec<String> = rows
        .iter()
        .map(|row| {
            (row.x + row.indent..row.x + row.width)
                .map(|x| frame.buffer[(x, row.y)].symbol())
                .collect()
        })
        .collect();
    assert!(rows.iter().all(|row| row.indent == 2), "after the prompt");
    assert_eq!(texts.join(" "), prompt, "every character is kept, in order");
    assert!(
        texts[0].chars().count() > usize::from(rows[0].width) * 3 / 2,
        "a row holds about twice as many characters as cells: {texts:?}"
    );
}

#[test]
fn test_overlay_covers_chat_prose_rows() {
    use crate::components::{Message, MessageRole};

    let mut app = crate::App::new();
    app.add_message(Message::new(MessageRole::User, "run the tests"));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    let draw = |terminal: &mut ratatui::Terminal<_>, app: &mut crate::App| {
        terminal
            .draw(|frame| super::render(frame, app, &test_theme()))
            .unwrap();
    };
    draw(&mut terminal, &mut app);
    assert_eq!(app.prose_rows().len(), 1);

    submit(&mut app, "/help");
    draw(&mut terminal, &mut app);
    assert!(app.prose_rows().is_empty(), "help covers the prompt");
}

#[test]
fn test_chat_reads_like_a_terminal_from_the_left_edge() {
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
    let screen = rows.join("\n");
    assert!(
        !screen.contains("YOU  ") && !screen.contains("AURA  "),
        "{screen}"
    );
    let user = rows.iter().position(|r| r.contains("→ hi")).unwrap();
    let reply = rows.iter().position(|r| r.contains("The task")).unwrap();
    // The prompt's frame takes a padding row and a border row below it,
    // then the gap between blocks.
    assert_eq!(reply, user + 4, "the reply follows the prompt: {screen}");

    let text_col = col(&rows[user], "→ hi");
    // Measured on the reply's row: the prompt's row has its frame's own
    // side borders.
    let border = rows[reply][..rows[reply].find("The task").unwrap()]
        .chars()
        .collect::<Vec<_>>()
        .iter()
        .rposition(|&c| c == '│')
        .unwrap();
    let chars: Vec<char> = rows[reply].chars().collect();
    let right_border = border + 1 + chars[border + 1..].iter().position(|&c| c == '│').unwrap();
    let inner = right_border - border - 1;
    let base = (inner - 4).min(110);
    let width = (base * 83 + 50) / 100;
    assert_eq!(
        text_col,
        border + 1 + (inner - width) / 2,
        "centered with equal padding on both sides: {screen}"
    );
    assert_eq!(col(&rows[reply], "The task"), text_col);
}

#[test]
fn test_user_message_is_framed_with_padding() {
    use crate::components::{Message, MessageRole};

    let theme = test_theme();
    let mut app = crate::App::new();
    app.add_message(Message::new(MessageRole::User, "hi"));
    app.add_message(Message::new(MessageRole::Assistant, "hello"));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
    terminal
        .draw(|frame| super::render(frame, &mut app, &theme))
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
    let user = rows.iter().position(|r| r.contains("→ hi")).unwrap();
    let text = rows[user].find("→ hi").unwrap();
    let text_col = rows[user][..text].chars().count();
    let left = text_col - 2;

    let at = |row: usize, x: usize| rows[row].chars().nth(x).unwrap();
    assert_eq!(at(user - 2, left), '╭', "top border:\n{screen}");
    assert_eq!(at(user + 2, left), '╰', "bottom border:\n{screen}");
    for row in user - 1..=user + 1 {
        assert_eq!(at(row, left), '│', "side border on row {row}:\n{screen}");
    }
    let right = rows[user - 2].chars().position(|c| c == '╮').unwrap();
    assert_eq!(at(user + 2, right), '╯', "{screen}");
    assert_eq!(at(user, right), '│', "{screen}");

    // The frame is not shaded: no cell in it, border included, carries
    // the surface background.
    #[allow(clippy::cast_possible_truncation)]
    let cell = |row: usize, x: usize| &buffer[(x as u16, row as u16)];
    for row in user - 2..=user + 2 {
        for x in left..=right {
            assert_ne!(
                cell(row, x).bg,
                theme.colors.surface,
                "({x}, {row}):\n{screen}"
            );
        }
    }
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
    assert!(rows[sandbox].contains("PLN"), "{}", rows[sandbox]);
    assert!(
        rows[sandbox + 3].contains("▸  src"),
        "{}",
        rows[sandbox + 3]
    );
    assert!(rows[sandbox + 4].contains("Cargo.toml"));
    assert_eq!(
        column(&rows[sandbox + 3], "▸  src"),
        kernel_border + 2,
        "tree rows share the Record panel's one-column gutter"
    );
}

#[test]
fn test_side_panel_header_matches_the_agents_header() {
    let mut app = crate::App::new();
    let rows = render_rows(&mut app, 150);
    let (tabs, widths) = header_bar(&rows, "SBX");
    assert!(
        rows[tabs].contains("PRC") && rows[tabs].contains("│ * │ ⤢ │"),
        "the tabs, then the * and ⤢ buttons: {}",
        rows[tabs]
    );
    assert_eq!(
        widths.len(),
        7,
        "the tabs and the two buttons fill the bar: {widths:?}"
    );
    assert_eq!(widths[5..], [3, 3]);
    assert!(
        widths[..5].iter().all(|w| w.abs_diff(widths[0]) <= 1),
        "{widths:?}"
    );
    assert_eq!(
        header_bar(&rows, "PROJECTS").0,
        header_bar(&rows, "KRN").0,
        "top bars line up"
    );

    let rows = render_rows(&mut app, 260);
    let (tabs, widths) = header_bar(&rows, "SANDBOX");
    assert!(rows[tabs].contains("PROCESSES"), "{}", rows[tabs]);
    assert_eq!(widths.len(), 7, "{widths:?}");
}

#[test]
fn test_side_panel_star_button_lists_the_whole_project() {
    use crate::app::SideScope;
    use crate::events::{AgentSummary, ProcessSummary, UiCommand};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

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
    let click = |app: &mut crate::App, column, row| {
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
    let screen = render_rows(&mut app, 150).join("\n");
    assert!(screen.contains("No processes for this agent."), "{screen}");
    assert!(screen.contains("The * button lists"), "{screen}");
    assert!(screen.contains("the project's 1."), "{screen}");
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
    let buffer = render_buffer(&mut app, 260);
    let row = row_of(&buffer, "PROCESSES");
    let (label, label_style) = find_cell(&buffer, row, "SANDBOX").unwrap();
    assert!(label_style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(label_style.bg, Some(Color::Reset), "active, unfocused");
    assert_eq!(label_style.fg, Some(theme.colors.foreground));
    let screen = render_rows(&mut app, 260).join("\n");
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
    let screen = render_rows(&mut app, 260).join("\n");
    assert!(!screen.contains("LIVE"), "{screen}");
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
    for _ in 0..3 {
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
    assert!(!screen.contains("PLN"));

    submit(&mut app, "/files");
    let screen = render_rows(&mut app, 150).join("\n");
    assert!(screen.contains("PLN"), "{screen}");
    assert!(!screen.contains("KRN"));
}

#[test]
fn test_plans_tab_lists_plans_with_their_todos() {
    use crate::app::PanelFocus;
    use crate::events::{PlanView, TodoState, TodoView, UiCommand};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let todo = |content: &str, state| TodoView {
        content: content.into(),
        state,
        assignee: None,
        task: None,
    };
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetPlans(vec![
        PlanView {
            path: ".aura/plans/auth_00000001.plan.md".into(),
            name: "Auth rework".into(),
            todos: vec![
                todo("Add session table", TodoState::Completed),
                todo("Write the login handler", TodoState::InProgress),
            ],
        },
        PlanView {
            path: ".aura/plans/parser_00000002.plan.md".into(),
            name: "Parser cleanup".into(),
            todos: vec![todo("Split the lexer", TodoState::Completed)],
        },
    ]));
    while app.focus() != PanelFocus::Sandbox {
        let _ = app.handle_key(KeyEvent::from(KeyCode::Tab));
    }
    for _ in 0..2 {
        let _ = app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    }
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
    // A todo's box sits under its plan's fold arrow.
    let last_column = |row: usize, glyph: char| {
        rows[row]
            .chars()
            .enumerate()
            .filter_map(|(i, c)| (c == glyph).then_some(i))
            .last()
            .unwrap_or_else(|| panic!("no {glyph} in {:?}", rows[row]))
    };
    assert_eq!(last_column(auth + 1, '☒'), last_column(auth, '▾'));
    assert_eq!(last_column(auth + 2, '▣'), last_column(auth, '▾'));
    let parser = row_with("Parser cleanup");
    assert_eq!(parser, auth + 3);
    assert!(rows[parser].contains("1/1"));
    assert!(
        !rows.join("\n").contains("Split the lexer"),
        "done plans start folded"
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
            context_window: None,
        }],
        current: "local/qwen3:4b".to_string(),
        route: None,
        user: user.map(str::to_string),
        plans: plans.iter().map(|p| (*p).to_string()).collect(),
        thinking: None,
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

#[test]
fn test_thinking_level_sits_after_the_model() {
    use crate::events::UiCommand;

    let mut app = app_with_model(None);
    let footer = |app: &mut crate::App| {
        render_rows(app, 120)
            .into_iter()
            .find(|row| row.contains("qwen3:4b"))
            .unwrap()
    };
    assert!(footer(&mut app).contains("qwen3:4b · auto thinking"));

    app.process_command(UiCommand::SetModels {
        options: app.models().to_vec(),
        current: "local/qwen3:4b".to_string(),
        route: None,
        user: None,
        plans: Vec::new(),
        thinking: Some("high".to_string()),
    });
    assert!(footer(&mut app).contains("qwen3:4b · high thinking"));
}

#[test]
fn test_thinking_level_outlasts_the_stats_after_a_turn() {
    use crate::events::{CallStats, UiCommand};

    let mut app = app_with_model(None);
    app.process_command(UiCommand::SetApproval(crate::ApprovalLevel::Ask));
    app.process_command(UiCommand::CallStats(CallStats {
        model: "local/qwen3:4b".to_string(),
        input_tokens: 4_000,
        cache_read_tokens: 120_000,
        cost_usd: Some(3.42),
        context_limit: Some(200_000),
        ..CallStats::default()
    }));
    let footer = |app: &mut crate::App, width| {
        render_rows(app, width)
            .into_iter()
            .find(|row| row.contains("qwen3:4b"))
            .expect("the footer shows the model")
    };
    let narrow = footer(&mut app, 120);
    assert!(
        narrow.contains("ASK · qwen3:4b · auto thinking"),
        "{narrow:?}"
    );
    assert!(
        !narrow.contains("context"),
        "the stats give way: {narrow:?}"
    );

    let wider = footer(&mut app, 160);
    assert!(
        wider.contains("ASK · qwen3:4b · auto thinking · 62% context used"),
        "{wider:?}"
    );
    assert!(
        !wider.contains("1 call"),
        "a later stat gives way: {wider:?}"
    );
}

#[test]
fn test_thinking_level_gives_way_before_the_model() {
    use crate::events::UiCommand;

    let mut app = app_with_model(None);
    app.process_command(UiCommand::SetApproval(crate::ApprovalLevel::Strict));
    app.process_command(UiCommand::SetMode(crate::PermissionMode::Plan));
    let footer = render_rows(&mut app, 120)
        .into_iter()
        .find(|row| row.contains("qwen3:4b"))
        .expect("the model stays when the footer is too narrow for both");
    assert!(!footer.contains("thinking"), "{footer:?}");
}

#[test]
fn test_approval_level_sits_before_the_model() {
    use crate::events::UiCommand;

    let mut app = app_with_model(None);
    let input_row = |app: &mut crate::App| {
        render_rows(app, 120)
            .into_iter()
            .find(|row| row.contains("qwen3:4b"))
            .unwrap()
    };
    assert!(!input_row(&mut app).contains("ASK"), "unknown until told");

    app.process_command(UiCommand::SetApproval(crate::ApprovalLevel::Yolo));
    assert!(input_row(&mut app).contains("YOLO · qwen3:4b"));

    app.process_command(UiCommand::SetApproval(crate::ApprovalLevel::Strict));
    app.process_command(UiCommand::SetMode(crate::PermissionMode::Plan));
    assert!(input_row(&mut app).contains("plan mode (/plan to leave) · STRICT · qwen3:4b"));
}

#[test]
fn test_hosted_panels_leave_their_title_bars_to_the_front_end() {
    use crate::app::{ChatTab, Pane, PaneSplits, PaneTab};

    let mut app = crate::App::new();
    app.set_host_chrome(true);
    let rows = render_rows(&mut app, 150);
    let screen = rows.join("\n");
    assert!(
        !screen.contains("CHAT") && !screen.contains("KRN"),
        "{screen}"
    );
    let panes: Vec<Pane> = app.pane_chrome().iter().map(|c| c.pane).collect();
    assert_eq!(panes, [Pane::Agents, Pane::Chat, Pane::Records, Pane::Side]);

    let chat = app.pane_chrome()[1].clone();
    assert_eq!(chat.active, Some(PaneTab::Chat(ChatTab::Chat)));
    let row = |y: u16| -> String {
        rows[usize::from(y)]
            .chars()
            .skip(usize::from(chat.bar.x))
            .take(usize::from(chat.bar.width))
            .collect()
    };
    assert!(row(chat.bar.y).starts_with('┌'), "{screen}");
    assert_eq!(row(chat.bar.y + 1).trim_matches('│').trim(), "", "{screen}");
    let joint = row(chat.bar.y + 2);
    assert!(joint.starts_with('├') && joint.ends_with('┤'), "{screen}");

    app.set_pane_splits(PaneSplits {
        agents: Some(0.1),
        ..PaneSplits::default()
    });
    render_rows(&mut app, 150);
    let [agents, chat] = [0, 1].map(|i| app.pane_chrome()[i].area);
    assert_eq!((agents.x, agents.width), (1, 15), "inside the edge column");
    assert_eq!(chat.x, agents.right() + 1, "a column between panels");

    app.set_pane_splits(PaneSplits {
        agents: Some(0.9),
        ..PaneSplits::default()
    });
    render_rows(&mut app, 150);
    assert!(app.pane_chrome()[1].area.width >= 30, "the chat keeps room");
}

/// Rows from the bottom of the screen to a one-row input: below it are the
/// input box's bottom border and the footer's two rows.
const INPUT_FROM_BOTTOM: usize = 3;

#[test]
fn test_side_panels_end_level_with_the_input_box() {
    let mut app = crate::App::new();
    let rows = render_rows(&mut app, 120);
    let input_bottom = rows.len() - INPUT_FROM_BOTTOM + 1;
    let bottom = &rows[input_bottom];
    assert!(bottom.starts_with(" └"), "{bottom:?}");
    assert!(bottom.trim_end().ends_with('┘'), "{bottom:?}");
    assert_eq!(bottom.matches('└').count(), 3, "{bottom:?}");
    for footer in &rows[input_bottom + 1..] {
        assert!(!footer.contains('│'), "{footer:?}");
    }
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

/// The chat panel's header tab row and its left and right border columns,
/// found from its CHAT | TERMINAL header bar (TRM when the bar is narrow).
fn chat_panel(rows: &[String]) -> (usize, usize, usize) {
    let tabs = rows
        .iter()
        .position(|r| r.contains("CHAT") && (r.contains("TERMINAL") || r.contains(" TRM ")))
        .unwrap_or_else(|| panic!("no chat header bar in:\n{}", rows.join("\n")));
    let row: Vec<char> = rows[tabs].chars().collect();
    let chat = rows[tabs][..rows[tabs].find("CHAT").unwrap()]
        .chars()
        .count();
    let left = row[..=chat].iter().rposition(|&c| c == '│').unwrap();
    let right = left
        + 1
        + rows[tabs - 1]
            .chars()
            .skip(left + 1)
            .position(|c| c == '┐')
            .unwrap();
    (tabs, left, right)
}

#[test]
fn test_slash_menu_spans_chat_width_above_input() {
    let mut app = app_with_model(None);
    type_text(&mut app, "/lo");
    let rows = render_rows(&mut app, 120);
    let n = rows.len();

    let (_, chat_left, chat_right) = chat_panel(&rows);

    let menu_bottom = &rows[n - 4];
    let chars: Vec<char> = menu_bottom.chars().collect();
    assert_eq!(chars[chat_left], '└', "{menu_bottom:?}");
    assert_eq!(chars[chat_right], '┘', "{menu_bottom:?}");
    assert!(menu_bottom.contains("Esc close"), "{menu_bottom:?}");

    assert!(rows[n - 7].contains("› /log "), "{:?}", rows[n - 7]);
    assert!(rows[n - 6].contains("  /login"), "{:?}", rows[n - 6]);
    assert!(rows[n - 6].contains("Login for cloud models"));
    assert!(rows[n - 5].contains("  /logout"), "{:?}", rows[n - 5]);
    assert!(rows[n - 8].contains(" COMMANDS "), "{:?}", rows[n - 8]);
    let menu_top: Vec<char> = rows[n - 8].chars().collect();
    assert_eq!(menu_top[chat_left], '┌');
    assert_eq!(menu_top[chat_right], '┐');

    type_text(&mut app, " ");
    let rows = render_rows(&mut app, 120);
    assert!(!rows.join("\n").contains(" COMMANDS "));
}

#[test]
fn test_mention_picker_lists_files_above_the_input() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "").unwrap();
    let mut app = app_with_model(None);
    app.process_command(crate::events::UiCommand::SetSandboxRoot(
        dir.path().to_path_buf(),
    ));
    type_text(&mut app, "look at @ma");
    let rows = render_rows(&mut app, 120);
    let n = rows.len();
    assert!(rows[n - 6].contains(" FILES "), "{:?}", rows[n - 6]);
    assert!(rows[n - 5].contains("› @src/main.rs"), "{:?}", rows[n - 5]);
    assert!(rows[n - 4].contains("Enter insert"), "{:?}", rows[n - 4]);

    type_text(&mut app, " ");
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(!screen.contains(" FILES "));
}

#[test]
fn test_input_shows_image_chips_and_pasted_newlines() {
    let theme = test_theme();
    let mut app = app_with_model(None);
    app.handle_paste("first\nsecond");
    app.attach_image(b"\x89PNG\r\n\x1a\nimage").unwrap();
    let rows = render_rows(&mut app, 120);
    let input_row = &rows[rows.len() - INPUT_FROM_BOTTOM];
    let first_row = &rows[rows.len() - INPUT_FROM_BOTTOM - 1];
    assert!(input_row.contains("│   second [image 1]"), "{input_row:?}");
    assert!(first_row.contains("│ → first "), "{first_row:?}");
    assert!(!first_row.contains('↵') && !input_row.contains('↵'));

    let spans = super::input::prose_spans("see @src/a.rs and [image 2] me@x", &theme);
    let picked: Vec<&str> = spans
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(picked, ["@src/a.rs", "[image 2]"]);

    submit(&mut app, "");
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(screen.contains("[image 1]"), "the chat shows the chip");
}

#[test]
fn test_help_lists_the_input_keys_without_clipping() {
    let mut app = crate::App::new();
    submit(&mut app, "/help");
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 50)).unwrap();
    terminal
        .draw(|frame| super::render(frame, &mut app, &test_theme()))
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
    for line in [
        "@path      Attach a file or folder",
        "Ctrl+V     Paste an image (or text)",
        "Ctrl+G     Edit the input in $EDITOR",
        "Press any key to close",
    ] {
        assert!(screen.contains(line), "{line:?} missing:\n{screen}");
    }
}

#[test]
fn test_agent_space_shows_permanent_agents_to_add() {
    use crate::events::{AgentDefinitionSummary, UiCommand};

    let mut app = app_with_model(None);
    app.process_command(UiCommand::SetAgentDefinitions(vec![
        AgentDefinitionSummary {
            name: "explorer".to_string(),
            description: "Reads code\nand more".to_string(),
            ..Default::default()
        },
        AgentDefinitionSummary {
            name: "reviewer".to_string(),
            description: "Reviews diffs".to_string(),
            ..Default::default()
        },
    ]));
    type_text(&mut app, "/agent ");
    let rows = render_rows(&mut app, 120);
    let n = rows.len();

    assert!(
        rows[n - 7].contains(" ADD AGENT TO PROJECT "),
        "{:?}",
        rows[n - 7]
    );
    assert!(
        rows[n - 6].contains("› explorer   Reads code"),
        "{:?}",
        rows[n - 6]
    );
    assert!(!rows[n - 6].contains("and more"));
    assert!(
        rows[n - 5].contains("  reviewer   Reviews diffs"),
        "{:?}",
        rows[n - 5]
    );
    assert!(rows[n - 4].contains("Enter add"), "{:?}", rows[n - 4]);
}

#[test]
fn test_header_shows_local_url_and_model_moves_to_the_footer() {
    let rows = render_with_user(None, 120);
    assert!(rows[0]
        .trim_end()
        .ends_with("● local  http://127.0.0.1:8080"));
    assert!(!rows[0].contains("qwen3:4b"));

    let n = rows.len();
    let (_, chat_left, chat_right) = chat_panel(&rows);
    let divider: Vec<char> = rows[n - 4].chars().collect();
    let box_bottom: Vec<char> = rows[n - 2].chars().collect();
    assert_eq!((divider[chat_left], divider[chat_right]), ('├', '┤'));
    assert_eq!((box_bottom[chat_left], box_bottom[chat_right]), ('└', '┘'));
    let above: Vec<char> = rows[n - 5].chars().collect();
    assert_eq!(
        (above[chat_left], above[chat_right]),
        ('│', '│'),
        "the chat panel runs down to the line above the input: {:?}",
        rows[n - 5]
    );

    let input_row = &rows[n - INPUT_FROM_BOTTOM];
    assert!(input_row.contains("│ →  Ask anything"), "{input_row:?}");
    assert!(!input_row.contains("]$ "), "{input_row:?}");
    assert!(!input_row.contains("qwen3:4b"), "{input_row:?}");

    let footer: String = rows[n - 1]
        .chars()
        .skip(chat_left)
        .take(chat_right - chat_left)
        .collect();
    assert!(footer.trim_start().starts_with("qwen3:4b"), "{footer:?}");
    assert!(footer.trim_end().ends_with("● Ready"), "{footer:?}");
    assert!(!footer.contains("/model"));
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
fn test_typing_sits_in_a_box_under_the_chat_panel() {
    let mut app = app_with_model(None);
    type_text(&mut app, "hello");
    let rows = render_rows(&mut app, 120);
    let input_row = &rows[rows.len() - INPUT_FROM_BOTTOM];

    let (_, chat_border, chat_right) = chat_panel(&rows);
    let chars: Vec<char> = input_row.chars().collect();
    let typing = chat_border + 4;
    let typed: String = chars[typing..typing + 5].iter().collect();
    let lead: String = chars[chat_border..typing].iter().collect();
    assert_eq!(typed, "hello", "{input_row:?}");
    assert_eq!(lead, "│ → ", "{input_row:?}");
    assert_eq!(chars[chat_right], '│', "{input_row:?}");
}

#[test]
fn test_long_input_wraps_inside_the_chat_and_grows_upward() {
    let mut app = app_with_model(None);
    let text = "word ".repeat(40);
    type_text(&mut app, text.trim_end());
    let (rows, (_, cursor_y)) = render_with_cursor(&mut app, 120);
    let n = rows.len();
    let (_, chat_left, chat_right) = chat_panel(&rows);

    assert!(rows[n - 1].contains("● Ready"), "{:?}", rows[n - 1]);
    let input_rows: Vec<&String> = rows[..n - 3]
        .iter()
        .rev()
        .take_while(|r| r.contains("word"))
        .collect();
    assert!(input_rows.len() > 1, "{rows:#?}");
    for row in &input_rows {
        let chars: Vec<char> = row.chars().collect();
        let beyond: String = chars[chat_right..].iter().collect();
        assert!(!beyond.contains("wo") && !beyond.contains("or"), "{row:?}");
        assert_eq!(chars[chat_left], '│', "{row:?}");
        assert_eq!(chars[chat_right], '│', "{row:?}");
    }
    assert_eq!(
        usize::from(cursor_y),
        n - INPUT_FROM_BOTTOM,
        "cursor on the last input row"
    );
}

#[test]
fn test_prompt_pushes_typing_right_without_gutter() {
    let mut app = app_with_model(None);
    submit_message(&mut app, "/swarm");
    type_text(&mut app, "hello");
    let rows = render_rows(&mut app, 120);
    let input_row = &rows[rows.len() - INPUT_FROM_BOTTOM];

    assert!(input_row.starts_with(" │ → hello"), "{input_row:?}");
}

/// `chars[from..to]` as a string.
fn columns(chars: &[char], from: usize, to: usize) -> String {
    chars[from..to].iter().collect()
}

#[test]
fn test_busy_input_shows_a_stop_button_at_the_right_edge() {
    let mut app = app_with_model(None);
    submit_message(&mut app, "hi");
    assert!(app.is_busy());
    let rows = render_rows(&mut app, 120);
    let y = rows.len() - INPUT_FROM_BOTTOM;
    let (_, chat_left, chat_right) = chat_panel(&rows);
    let chars: Vec<char> = rows[y].chars().collect();
    let in_box = columns(&chars, chat_left, chat_right + 1);
    assert!(in_box.ends_with(" ■ Stop │"), "{in_box:?}");
    assert!(in_box.starts_with("│ → "), "{in_box:?}");

    let stop = app.input_stop_area().expect("the button's cells");
    assert_eq!((usize::from(stop.y), stop.width, stop.height), (y, 6, 1));
    let (x, end) = (usize::from(stop.x), usize::from(stop.right()));
    assert_eq!(columns(&chars, x, end), "■ Stop");
    assert_eq!(end + 1, chat_right, "a column of padding before the border");
}

#[test]
fn test_idle_input_has_no_stop_button() {
    let mut app = app_with_model(None);
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(!screen.contains("Stop"), "{screen}");
    assert_eq!(app.input_stop_area(), None);

    // It comes with a turn and goes with it.
    submit_message(&mut app, "hi");
    render_rows(&mut app, 120);
    assert!(app.input_stop_area().is_some());
    app.cancel();
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(!screen.contains("■ Stop"), "{screen}");
    assert_eq!(app.input_stop_area(), None);
}

#[test]
fn test_typing_beside_the_stop_button_wraps_before_it() {
    let mut app = app_with_model(None);
    submit_message(&mut app, "hi");
    let typed = "abcdefghij".repeat(10);
    type_text(&mut app, &typed);
    let rows = render_rows(&mut app, 120);
    let last = rows.len() - INPUT_FROM_BOTTOM;
    let (_, chat_left, chat_right) = chat_panel(&rows);
    let typing = chat_left + 4;
    let stop = app.input_stop_area().expect("the button's cells");
    let (x, y) = (usize::from(stop.x), usize::from(stop.y));
    assert!(y < last, "the text wraps onto more rows: {rows:#?}");

    let first: Vec<char> = rows[y].chars().collect();
    assert_eq!(columns(&first, x, x + 6), "■ Stop", "{:?}", rows[y]);
    assert_eq!(first[x - 1], ' ', "a gap before the button");
    assert!(first[x - 2].is_ascii_lowercase(), "{:?}", rows[y]);

    let mut shown = String::new();
    for row in &rows[y..=last] {
        let chars: Vec<char> = row.chars().collect();
        shown += columns(&chars, typing, x - 1).trim_end();
        if row != &rows[y] {
            let beyond = columns(&chars, x - 1, chat_right);
            assert!(beyond.trim().is_empty(), "under the button: {row:?}");
        }
    }
    assert_eq!(shown, typed, "every letter is drawn left of the button");
}

#[test]
fn test_typed_newlines_grow_the_input_box_a_row_each() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let theme = test_theme();
    let area = Rect::new(0, 0, 60, 20);
    let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
    let mut app = app_with_model(None);
    type_text(&mut app, "a");
    assert_eq!(super::input::input_rows(area, &app, &theme, 20), 1);
    for c in ["b", "c"] {
        app.handle_key(shift_enter);
        type_text(&mut app, c);
    }
    assert_eq!(app.input(), "a\nb\nc");
    assert_eq!(super::input::input_rows(area, &app, &theme, 20), 3);

    let (rows, (x, y)) = render_with_cursor(&mut app, 120);
    let n = rows.len();
    let (_, chat_left, chat_right) = chat_panel(&rows);
    let typing = chat_left + 4;
    let in_box = |row: &String| -> String {
        let chars: Vec<char> = row.chars().collect();
        columns(&chars, chat_left, chat_right + 1)
    };
    assert!(in_box(&rows[n - 5]).starts_with("│ → a "), "{rows:#?}");
    assert!(in_box(&rows[n - 4]).starts_with("│   b "), "{rows:#?}");
    assert!(in_box(&rows[n - 3]).starts_with("│   c "), "{rows:#?}");
    let divider: Vec<char> = rows[n - 6].chars().collect();
    assert_eq!(
        (divider[chat_left], divider[chat_right]),
        ('├', '┤'),
        "the box starts three rows up: {:?}",
        rows[n - 6]
    );
    assert_eq!((usize::from(x), usize::from(y)), (typing + 1, n - 3));

    // A trailing break opens an empty row with the cursor at its start.
    app.handle_key(shift_enter);
    assert_eq!(super::input::input_rows(area, &app, &theme, 20), 4);
    let (rows, (x, y)) = render_with_cursor(&mut app, 120);
    let n = rows.len();
    assert!(in_box(&rows[n - 4]).starts_with("│   c "), "{rows:#?}");
    assert!(in_box(&rows[n - 3]).trim_matches('│').trim().is_empty());
    assert_eq!((usize::from(x), usize::from(y)), (typing, n - 3));
    assert!(!rows.join("\n").contains('↵'));
}

/// Shift+Enter on an empty prompt opens a second row, wherever the focus
/// is and whether or not a turn is running.
#[test]
fn test_shift_enter_on_an_empty_prompt_grows_the_box() {
    use crate::app::PanelFocus;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let theme = test_theme();
    let area = Rect::new(0, 0, 60, 20);
    let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
    for focus in [
        PanelFocus::Chat,
        PanelFocus::Swarm,
        PanelFocus::Records,
        PanelFocus::Sandbox,
    ] {
        for busy in [false, true] {
            let mut app = app_with_model(None);
            if busy {
                submit_message(&mut app, "hi");
                assert!(app.is_busy());
            }
            render_with_cursor(&mut app, 120);
            for _ in 0..4 {
                if app.focus() == focus {
                    break;
                }
                app.handle_key(KeyEvent::from(KeyCode::Tab));
            }
            assert_eq!(app.focus(), focus);
            assert_eq!(super::input::input_rows(area, &app, &theme, 20), 1);
            let (before, _) = render_with_cursor(&mut app, 120);

            app.handle_key(shift_enter);
            assert_eq!(app.input(), "\n", "{focus:?} busy={busy}");
            assert_eq!(
                super::input::input_rows(area, &app, &theme, 20),
                2,
                "{focus:?} busy={busy}"
            );
            app.handle_key(shift_enter);
            assert_eq!(super::input::input_rows(area, &app, &theme, 20), 3);

            let (rows, (_, y)) = render_with_cursor(&mut app, 120);
            let divider_row = |rows: &[String]| {
                let (_, chat_left, _) = chat_panel(rows);
                rows.iter()
                    .rposition(|row| row.chars().nth(chat_left) == Some('├'))
                    .unwrap()
            };
            assert_eq!(
                divider_row(&before) - divider_row(&rows),
                2,
                "the box grows two rows: {rows:#?}"
            );
            // The cursor hides while the agent thinks.
            if focus == PanelFocus::Chat && !busy {
                assert_eq!(usize::from(y), divider_row(&rows) + 3, "busy={busy}");
            }
            assert!(!rows.join("\n").contains("Add a follow-up"));
            assert!(!rows.join("\n").contains("Ask anything"));
        }
    }
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
fn test_terminal_types_at_the_input_line_after_the_shell_prompt() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = app_with_model(None);
    type_text(&mut app, "$ls");
    let (rows, (x, y)) = render_with_cursor(&mut app, 120);

    let (_, chat_left, _) = chat_panel(&rows);
    let input_y = rows.len() - INPUT_FROM_BOTTOM;
    let typed: String = rows[input_y].chars().skip(chat_left + 2).collect();
    let prompt = "[n3o@arch-aura aura-harness]$ ";
    assert!(typed.starts_with(&format!("{prompt}ls")), "{typed:?}");
    assert_eq!(
        (usize::from(x), usize::from(y)),
        (chat_left + 2 + prompt.len() + 2, input_y)
    );
    let pane = rows[..input_y].join("\n");
    assert!(!pane.contains("]$ "), "{pane}");
    assert!(pane.contains("Esc returns to chat"), "{pane}");

    app.handle_key(KeyEvent::from(KeyCode::Backspace));
    app.handle_key(KeyEvent::from(KeyCode::Backspace));
    app.handle_key(KeyEvent::from(KeyCode::Backspace));
    let rows = render_rows(&mut app, 120);
    let input_row = &rows[rows.len() - INPUT_FROM_BOTTOM];
    assert!(input_row.contains("→ "), "{input_row:?}");
    assert!(!rows.join("\n").contains("]$ "));
}

#[test]
fn test_terminal_input_is_highlighted_as_a_command() {
    let theme = test_theme();
    let mut app = app_with_model(None);
    type_text(&mut app, "$ls -la");
    let buffer = render_buffer(&mut app, 120);
    let y = buffer.area.height - 3;

    let (_, program) = find_cell(&buffer, y, "ls -la").unwrap();
    assert_eq!(program.fg, Some(theme.colors.success));
    let (_, flag) = find_cell(&buffer, y, "-la").unwrap();
    assert_eq!(flag.fg, Some(theme.colors.warning));
}

#[test]
fn test_chat_panel_tabs_switch_between_chat_and_terminal() {
    use crate::app::ChatTab;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    let mut app = app_with_model(None);
    let rows = render_rows(&mut app, 120);
    let (tabs, left, right) = chat_panel(&rows);
    let first = ChatTab::layout(u16::try_from(right - left + 1 - 4).unwrap()).cells[0];
    let divider = left + usize::from(first.start + first.width);
    assert_eq!(rows[tabs - 1].chars().nth(divider), Some('┬'));
    assert_eq!(rows[tabs + 1].chars().nth(divider), Some('┴'));
    assert_eq!(rows[tabs + 2].chars().nth(left), Some('┌'), "body below");
    let end: String = rows[tabs].chars().skip(right - 4).take(5).collect();
    assert_eq!(end, "│ ⤢ │", "the expand button ends the bar");

    let theme = test_theme();
    let active = |app: &mut crate::App, label: &str| {
        let buffer = render_buffer(app, 120);
        let y = u16::try_from(tabs).unwrap();
        let (_, style) = find_cell(&buffer, y, label).unwrap();
        style.bg == Some(theme.colors.selection) && style.fg == Some(theme.colors.on_selection)
    };
    assert!(active(&mut app, "CHAT"));
    assert!(!active(&mut app, "TERMINAL"));

    type_text(&mut app, "$");
    assert_eq!(app.chat_tab(), ChatTab::Terminal);
    assert!(active(&mut app, "TERMINAL"));
    assert!(!active(&mut app, "CHAT"));

    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    assert_eq!(app.chat_tab(), ChatTab::Chat);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(app.chat_tab(), ChatTab::Terminal);

    let chat_label = u16::try_from(left + 2).unwrap();
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.handle_mouse(MouseEvent {
            kind,
            column: chat_label,
            row: u16::try_from(tabs).unwrap(),
            modifiers: KeyModifiers::empty(),
        });
    }
    assert_eq!(app.chat_tab(), ChatTab::Chat);
    assert!(app.selection().is_none());
}

#[test]
fn test_hovered_agent_row_shows_an_archive_button_left_of_its_time() {
    use crate::app::HoverTarget;
    use crate::events::{AgentSummary, UiCommand};
    use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};

    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![
        AgentSummary {
            id: "aa".to_string(),
            name: "Terminal Agent".to_string(),
            last_active_ms: Some(super::now_ms() - 3 * 60 * 1000),
            ..Default::default()
        },
        AgentSummary {
            id: "bb".to_string(),
            name: "Researcher".to_string(),
            ..Default::default()
        },
    ]));
    let char_col = |row: &str, needle: &str| {
        let i = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
        row[..i].chars().count()
    };
    let glyph_at = |row: &str, column: usize| row.chars().nth(column);
    let moved = |column: usize, row: usize| MouseEvent {
        kind: MouseEventKind::Moved,
        column: u16::try_from(column).unwrap(),
        row: u16::try_from(row).unwrap(),
        modifiers: KeyModifiers::empty(),
    };

    let before = render_rows(&mut app, 120);
    let line = before
        .iter()
        .position(|r| r.contains("Terminal Agent"))
        .expect("agent row");
    assert!(!before[line].contains('✕'), "{}", before[line]);
    let time = char_col(&before[line], " 3m ") + 1;
    let name = char_col(&before[line], "Terminal Agent");

    // Hovering the row anywhere shows ` ✕` just left of ` 3m `, and no
    // other button.
    app.handle_mouse(moved(name, line));
    let row_index = match app.hover() {
        Some(HoverTarget::ProjectRow(index)) => index,
        other => panic!("{other:?}"),
    };
    let hovered = render_rows(&mut app, 120);
    assert!(hovered[line].contains(" ✕ 3m "), "{}", hovered[line]);
    assert!(!hovered[line].contains('↗'), "{}", hovered[line]);
    assert_eq!(glyph_at(&hovered[line], time - 2), Some('✕'));
    assert_eq!(glyph_at(&hovered[line], time - 4), Some(' '));
    assert_eq!(char_col(&hovered[line], " 3m ") + 1, time, "the time stays");

    // The glyph is where the hit test puts the button; left of it is the
    // row.
    app.handle_mouse(moved(time - 2, line));
    assert_eq!(app.hover(), Some(HoverTarget::ArchiveAgent(row_index)));
    let on_button = render_rows(&mut app, 120);
    assert_eq!(glyph_at(&on_button[line], time - 2), Some('✕'));
    app.handle_mouse(moved(time - 4, line));
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(row_index)));

    // An agent with no time has its archive button one column in from
    // the edge.
    let other = before
        .iter()
        .position(|r| r.contains("Researcher"))
        .expect("agent row");
    let right = char_col(&before[other], "Researcher") + "Researcher".len();
    let edge = before[other][..]
        .chars()
        .enumerate()
        .skip(right)
        .find(|&(_, c)| c == '│')
        .map(|(i, _)| i)
        .expect("panel border");
    app.handle_mouse(moved(edge - 4, other));
    assert!(matches!(app.hover(), Some(HoverTarget::ProjectRow(_))));
    // Off the button it stays muted like the time; on it, it lights up.
    let colors = test_theme().colors;
    let row = u16::try_from(other).unwrap();
    let buffer = render_buffer(&mut app, 120);
    let (_, archive) = find_cell(&buffer, row, "✕").expect("archive button");
    assert_eq!(archive.fg, Some(colors.muted));
    app.handle_mouse(moved(edge - 2, other));
    assert!(matches!(app.hover(), Some(HoverTarget::ArchiveAgent(_))));
    let no_time = render_rows(&mut app, 120);
    assert_eq!(glyph_at(&no_time[other], edge - 2), Some('✕'));
    let buffer = render_buffer(&mut app, 120);
    let (_, archive) = find_cell(&buffer, row, "✕").expect("archive button");
    assert_eq!(archive.fg, Some(colors.foreground));

    // Off the rows, they look as they did.
    app.handle_mouse(moved(60, 0));
    let after = render_rows(&mut app, 120);
    assert_eq!(after[line], before[line]);
    assert_eq!(after[other], before[other]);
}

#[test]
fn test_expand_button_fills_the_screen_with_the_chat_panel() {
    use crate::app::{ChatTab, HoverTarget};
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    let mut app = app_with_model(None);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(app.chat_tab(), ChatTab::Graph);
    let rows = render_rows(&mut app, 120);
    let (tabs, left, right) = chat_panel(&rows);
    assert!(left > 2, "the agents panel is beside the chat");
    let mouse = |kind, column: usize| MouseEvent {
        kind,
        column: u16::try_from(column).unwrap(),
        row: u16::try_from(tabs).unwrap(),
        modifiers: KeyModifiers::empty(),
    };
    app.handle_mouse(mouse(MouseEventKind::Moved, right - 2));
    assert_eq!(app.hover(), Some(HoverTarget::ExpandChat));
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), right - 2));
    assert!(app.chat_expanded());
    assert_eq!(app.chat_tab(), ChatTab::Graph, "the tab stays");

    let rows = render_rows(&mut app, 120);
    let (tabs, wide_left, wide_right) = chat_panel(&rows);
    assert_eq!((wide_left, wide_right), (1, 118), "{}", rows.join("\n"));
    assert!(!rows.join("\n").contains("KRN"), "the other panels hide");
    let end: String = rows[tabs].chars().skip(wide_right - 4).take(5).collect();
    assert_eq!(end, "│ ⤡ │", "the button now restores");

    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    assert!(!app.chat_expanded(), "Ctrl+X restores it too");
    let rows = render_rows(&mut app, 120);
    assert_eq!(chat_panel(&rows), (tabs, left, right));
}

/// The side panel's header tab row and its left and right border
/// columns, found from its SBX tab.
fn side_panel(rows: &[String]) -> (usize, usize, usize) {
    let tabs = rows
        .iter()
        .position(|r| r.contains("SBX"))
        .unwrap_or_else(|| panic!("no side panel bar in:\n{}", rows.join("\n")));
    let row: Vec<char> = rows[tabs].chars().collect();
    let sbx = rows[tabs][..rows[tabs].find("SBX").unwrap()]
        .chars()
        .count();
    let left = row[..=sbx].iter().rposition(|&c| c == '│').unwrap();
    let top: Vec<char> = rows[tabs - 1].chars().collect();
    let right = left + top[left..].iter().position(|&c| c == '┐').unwrap();
    (tabs, left, right)
}

#[test]
fn test_expand_button_maximizes_the_side_panel_over_the_record_panel() {
    use crate::app::{HoverTarget, PanelFocus};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let mut app = crate::App::new();
    let rows = render_rows(&mut app, 120);
    let (kernel, _) = header_bar(&rows, "KRN");
    let (tabs, left, right) = side_panel(&rows);
    assert!(
        tabs > kernel,
        "the side panel starts below the Record panel"
    );
    let buffer = render_buffer(&mut app, 120);
    let row = u16::try_from(tabs).unwrap();
    let (expand, idle) = find_cell(&buffer, row, "⤢").expect("the expand button is drawn");
    assert_eq!(idle.fg, Some(test_theme().colors.muted));
    let mouse = |kind| MouseEvent {
        kind,
        column: expand,
        row,
        modifiers: KeyModifiers::empty(),
    };
    app.handle_mouse(mouse(MouseEventKind::Moved));
    assert_eq!(app.hover(), Some(HoverTarget::ExpandSide));
    let buffer = render_buffer(&mut app, 120);
    let lit = buffer[(expand, row)].style();
    assert_eq!(lit.bg, Some(test_theme().colors.surface_hover));
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left)));
    assert!(app.side_expanded());
    assert_eq!(app.focus(), PanelFocus::Sandbox);

    let rows = render_rows(&mut app, 120);
    let screen = rows.join("\n");
    assert!(!screen.contains("KRN"), "the Record panel hides: {screen}");
    assert_eq!(
        side_panel(&rows),
        (kernel, left, right),
        "the side panel covers the Record panel's room exactly"
    );
    let buffer = render_buffer(&mut app, 120);
    let row = u16::try_from(kernel).unwrap();
    assert!(
        find_cell(&buffer, row, "⤡").is_some(),
        "the button now restores"
    );
    let (restore, _) = find_cell(&buffer, row, "⤡").unwrap();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: restore,
        row,
        modifiers: KeyModifiers::empty(),
    });
    assert!(!app.side_expanded());
    let rows = render_rows(&mut app, 120);
    assert_eq!(header_bar(&rows, "KRN").0, kernel);
    assert_eq!(side_panel(&rows), (tabs, left, right));
}

#[test]
fn test_hosted_side_bar_tells_the_front_end_it_maximizes() {
    use crate::app::Pane;

    let mut app = crate::App::new();
    app.set_host_chrome(true);
    let side_expand = |app: &mut crate::App| {
        render_rows(app, 120);
        app.pane_chrome()
            .iter()
            .find(|chrome| chrome.pane == Pane::Side)
            .map(|chrome| (chrome.expand, chrome.scope))
    };
    assert_eq!(side_expand(&mut app), Some((Some(false), Some(false))));
    app.expand_pane(Pane::Side);
    assert_eq!(side_expand(&mut app), Some((Some(true), Some(false))));
    assert!(
        app.pane_chrome()
            .iter()
            .all(|chrome| chrome.pane != Pane::Records),
        "the Record panel is not drawn under it"
    );
}

#[test]
fn test_long_command_wraps_at_the_last_column() {
    let mut app = app_with_model(None);
    app.set_shell_prompt("$ ");
    submit_message(&mut app, "/swarm");
    let command = format!("echo {}", "x".repeat(200));
    submit_message(&mut app, &format!("${command}"));
    let rows = render_rows(&mut app, 120);

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
}

#[test]
fn test_pane_shows_only_the_running_command_and_input_keeps_the_prompt() {
    let mut app = app_with_model(None);
    app.set_shell_prompt("[u@h d]$ ");
    submit_message(&mut app, "$sleep 5");
    let rows = render_rows(&mut app, 120);
    let (_, chat_left, _) = chat_panel(&rows);
    let input_y = rows.len() - INPUT_FROM_BOTTOM;
    let pane = rows[..input_y].join("\n");
    assert_eq!(pane.matches("[u@h d]$ ").count(), 1, "{pane}");
    assert!(pane.contains("[u@h d]$ sleep 5"), "{pane}");
    assert!(!pane.contains("running sleep"), "{pane}");
    let typed: String = rows[input_y].chars().skip(chat_left + 2).collect();
    assert!(typed.starts_with("[u@h d]$ "), "{:?}", rows[input_y]);
}

#[test]
fn test_user_prompt_renders_markdown_under_its_prompt() {
    use crate::components::{Message, MessageRole};

    let mut app = app_with_model(None);
    app.add_message(Message::new(
        MessageRole::User,
        "Read-only. Use `git show` here.\n\n- **first** item\n- second item",
    ));
    let screen = render_rows(&mut app, 100).join("\n");
    assert!(
        screen.contains("→ Read-only. Use git show here."),
        "{screen}"
    );
    assert!(screen.contains("  • first item"), "{screen}");
    assert!(screen.contains("  • second item"), "{screen}");
    assert!(!screen.contains("**"), "{screen}");
    assert!(!screen.contains('`'), "{screen}");
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
    assert_eq!(app.chat_tab(), crate::app::ChatTab::Terminal);
    assert!(screen.contains("echo hi"), "{screen}");
    assert!(!screen.contains("hello agent"), "{screen}");

    app.handle_key(KeyEvent::from(KeyCode::Esc));
    let screen = render_rows(&mut app, 120).join("\n");
    assert_eq!(app.chat_tab(), crate::app::ChatTab::Chat);
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
    // The outer borders run through columns 1 and width - 2; the header
    // keeps a column inside them, as the footer does inside the chat's.
    let inner_right = usize::from(width) - 3;

    assert!(rows[0].starts_with("  AURA"), "{:?}", rows[0]);
    assert_eq!(last_col(&rows[0]), inner_right, "{:?}", rows[0]);
    let (_, _, chat_right) = chat_panel(&rows);
    let status: Vec<char> = rows[rows.len() - 1].chars().collect();
    assert_eq!(status[chat_right - 1], 'y', "{status:?}");
    assert_eq!(status[chat_right], ' ', "{status:?}");
}

#[test]
fn test_notification_sits_under_the_right_column_leaving_the_panels_alone() {
    let mut app = app_with_model(None);
    let before = render_rows(&mut app, 120);
    app.notify_copied(4, &Ok(()));
    let rows = render_rows(&mut app, 120);

    let n = rows.len();
    assert_eq!(rows[..n - 1], before[..n - 1], "no panel is covered");
    let (_, _, chat_right) = chat_panel(&rows);
    let footer: Vec<char> = rows[n - 1].chars().collect();
    let notice: String = footer[chat_right + 1..].iter().collect();
    assert!(
        notice.trim_end().ends_with("✓ Copied 4 characters"),
        "{notice:?}"
    );
    assert_eq!(
        rows[n - 1].trim_end().chars().count(),
        119,
        "{:?}",
        rows[n - 1]
    );
    let chat_footer: String = footer[..=chat_right].iter().collect();
    assert_eq!(
        chat_footer,
        before[n - 1]
            .chars()
            .take(chat_right + 1)
            .collect::<String>(),
        "the footer stays visible"
    );
}

fn long_warning(app: &mut crate::App) {
    let words: Vec<String> = (0..40).map(|i| format!("w{i}")).collect();
    app.notification = Some((words.join(" "), crate::app::NotificationType::Warning));
    app.tick();
}

#[test]
fn test_long_notification_wraps_over_the_bottom_third_of_the_lowest_right_panel() {
    let mut app = app_with_model(None);
    let before = render_rows(&mut app, 120);
    long_warning(&mut app);
    let rows = render_rows(&mut app, 120);

    let n = rows.len();
    let (_, _, chat_right) = chat_panel(&rows);
    let right = |row: &str| row.chars().skip(chat_right + 1).collect::<String>();
    let top = rows.iter().position(|row| row.contains("SBX")).unwrap() - 1;
    let bottom = n - 2;
    let third = (bottom + 1 - top) / 3;
    let changed: Vec<usize> = (0..n).filter(|&y| rows[y] != before[y]).collect();
    assert_eq!(changed.len(), third, "{changed:?}");
    assert!(
        changed.iter().all(|&y| y >= bottom - third && y < bottom),
        "only the bottom third of the panel, inside its border: {changed:?}"
    );

    let rule = right(&rows[changed[0]]);
    assert!(rule.trim().starts_with('├'), "{rule:?}");
    assert!(rule.trim().ends_with('┤'), "{rule:?}");
    assert!(right(&rows[changed[1]]).contains("! w0 w1 w2"), "{rows:?}");
    assert!(right(&rows[changed[2]]).contains("w7"), "wraps: {rows:?}");
    assert_eq!(rows[n - 1], before[n - 1], "the footer row stays clear");
}

#[test]
fn test_wrapped_notification_scrolls_up_to_its_end_and_stays_until_read() {
    let mut app = app_with_model(None);
    long_warning(&mut app);
    let notice = |rows: &[String]| rows[rows.len() - 5..rows.len() - 2].join("\n");
    let rows = render_rows(&mut app, 120);
    assert!(notice(&rows).contains("! w0 "), "{rows:?}");
    assert!(!notice(&rows).contains("w39"), "{rows:?}");

    let (_, until) = app.notification_shown.as_ref().unwrap();
    let since = app.notification_since.unwrap();
    assert!(
        *until > since + std::time::Duration::from_secs(5),
        "held for the scroll"
    );

    let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(60);
    app.notification_since = Some(long_ago);
    let rows = render_rows(&mut app, 120);
    let shown = notice(&rows);
    assert!(shown.contains("w39"), "ends at the end: {shown}");
    assert!(shown.contains("! "), "the icon stays: {shown}");
    assert!(!shown.contains("w0 "), "{shown}");
}

#[test]
fn test_long_notification_scrolls_along_the_footer_without_a_right_column() {
    let mut app = app_with_model(None);
    app.record_panel_visible = false;
    app.sandbox_panel_visible = false;
    long_warning(&mut app);
    let rows = render_rows(&mut app, 120);
    let last = rows.last().unwrap();
    assert!(last.contains("! w0 w1 w2"), "starts at the start: {last:?}");
    assert!(!last.contains("w39"), "{last:?}");

    let (_, until) = app.notification_shown.as_ref().unwrap();
    let since = app.notification_since.unwrap();
    assert!(
        *until > since + std::time::Duration::from_secs(5),
        "held for the scroll"
    );

    let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(60);
    app.notification_since = Some(long_ago);
    let rows = render_rows(&mut app, 120);
    let last = rows.last().unwrap();
    assert!(
        last.contains("w38 w39  ● Ready"),
        "ends at the end: {last:?}"
    );
    assert!(last.contains("! "), "the icon stays: {last:?}");
    assert!(!last.contains("w0 "), "{last:?}");
}

#[test]
fn test_notification_uses_the_footer_gap_without_a_right_column() {
    let mut app = app_with_model(None);
    app.record_panel_visible = false;
    app.sandbox_panel_visible = false;
    app.notify_copied(4, &Ok(()));
    let rows = render_rows(&mut app, 120);

    let footer = rows.last().unwrap();
    assert!(footer.contains("✓ Copied 4 characters"), "{footer:?}");
    assert!(footer.contains("qwen3:4b"), "the stats stay: {footer:?}");
    assert!(footer.trim_end().ends_with("Ready"), "{footer:?}");
}

#[test]
fn test_header_shows_logged_in_user() {
    let rows = render_with_user(Some("alice"), 120);
    assert!(rows[0]
        .trim_end()
        .ends_with("● alice (zOS)  http://127.0.0.1:8080"));
    assert!(!rows[0].contains("local"));
}

#[test]
fn test_header_shows_linked_plans_without_zos() {
    let rows = render_rows(&mut app_with_auth(None, &["Claude Max 20x"]), 120);
    assert!(
        rows[0]
            .trim_end()
            .ends_with("● CLAUDE MAX 20X  http://127.0.0.1:8080"),
        "{:?}",
        rows[0]
    );
    assert!(!rows[0].contains("local"));
}

#[test]
fn test_header_shows_zos_user_and_plans() {
    let rows = render_rows(
        &mut app_with_auth(Some("alice"), &["ChatGPT Pro", "Claude"]),
        120,
    );
    assert!(
        rows[0]
            .trim_end()
            .ends_with("● alice (zOS) · CHATGPT PRO · CLAUDE  http://127.0.0.1:8080"),
        "{:?}",
        rows[0]
    );
}

#[test]
fn test_input_shows_gate_instead_of_its_pick() {
    use crate::events::{ModelOptionView, UiCommand};

    let mut app = app_with_model(None);
    app.process_command(UiCommand::SetModels {
        options: vec![ModelOptionView {
            id: "gate".to_string(),
            label: "Gate".to_string(),
            local: false,
            installed: true,
            group: "Gate".to_string(),
            provider: None,
            available: true,
            context_window: None,
        }],
        current: "gate".to_string(),
        route: Some("claude-sonnet-4-6 (claude plan)".to_string()),
        user: None,
        plans: Vec::new(),
        thinking: None,
    });
    let rows = render_rows(&mut app, 120);
    let status = &rows[rows.len() - 1];
    assert!(status.contains("GATE"), "{status:?}");
    assert!(status.contains("● Ready"), "{status:?}");
    assert!(!status.contains("claude-sonnet-4-6"), "{status:?}");
}

#[test]
fn test_status_line_under_gate_counts_models_instead_of_context() {
    use crate::events::{CallStats, ModelOptionView, UiCommand};

    let mut app = app_with_model(None);
    app.process_command(UiCommand::SetModels {
        options: vec![ModelOptionView {
            id: "gate".to_string(),
            label: "Gate".to_string(),
            local: false,
            installed: true,
            group: "Gate".to_string(),
            provider: None,
            available: true,
            context_window: None,
        }],
        current: "gate".to_string(),
        route: None,
        user: None,
        plans: Vec::new(),
        thinking: None,
    });
    for model in ["aura-claude-haiku-4-5", "aura-claude-sonnet-4-6"] {
        app.process_command(UiCommand::CallStats(CallStats {
            model: model.to_string(),
            input_tokens: 40_000,
            cost_usd: Some(0.1),
            context_limit: Some(200_000),
            ..CallStats::default()
        }));
    }
    let rows = render_rows(&mut app, 160);
    let status = &rows[rows.len() - 1];
    assert!(!status.contains("context"), "{status:?}");
    assert!(status.contains("$0.200 · 2 models · 2 calls"), "{status:?}");
}

#[test]
fn test_status_line_on_one_model_measures_context_against_its_window() {
    use crate::events::{CallStats, ModelOptionView, UiCommand};

    let mut app = app_with_model(None);
    app.process_command(UiCommand::CallStats(CallStats {
        model: "aura-claude-sonnet-4-6".to_string(),
        input_tokens: 40_000,
        context_limit: Some(1_000_000),
        ..CallStats::default()
    }));
    app.process_command(UiCommand::SetModels {
        options: vec![ModelOptionView {
            id: "aura-claude-haiku-4-5".to_string(),
            label: "Claude Haiku 4.5".to_string(),
            local: false,
            installed: true,
            group: "Cloud (aura-router)".to_string(),
            provider: Some("zos".to_string()),
            available: true,
            context_window: Some(200_000),
        }],
        current: "aura-claude-haiku-4-5".to_string(),
        route: None,
        user: None,
        plans: Vec::new(),
        thinking: None,
    });
    let rows = render_rows(&mut app, 160);
    let status = &rows[rows.len() - 1];
    assert!(
        status.contains("Claude Haiku 4.5 · auto thinking · 20% context used"),
        "{status:?}"
    );
    assert!(!status.contains("model"), "{status:?}");
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
    let rows = render_rows(&mut app, 120);
    let chat = rows.join("\n");
    assert!(!chat.contains("AURA  "), "{chat}");
    let working = rows.iter().find(|r| r.contains("⬢ Thinking…")).unwrap();
    let prompt = rows.iter().find(|r| r.contains("→ hi")).unwrap();
    assert_eq!(
        working.chars().position(|c| c == '⬢'),
        prompt.chars().position(|c| c == '→'),
        "the hexagon starts at the prompt's column: {chat}"
    );

    app.process_command(UiCommand::StartStreaming);
    assert!(render_rows(&mut app, 120).join("\n").contains("Thinking…"));

    app.process_command(UiCommand::AppendText("hello".to_string()));
    assert!(!render_rows(&mut app, 120).join("\n").contains("Thinking…"));

    app.process_command(UiCommand::Complete);
    assert!(app.working_label().is_none());
}

/// The chat row closing a turn whose reply came from `model` with
/// `usage`, drawn `width` columns wide, from its `[` to its `]`.
fn reply_footer_row(model: &str, usage: Option<CallUsage>, width: u16) -> String {
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    submit_message(&mut app, "hi");
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("hello".to_string()));
    app.process_command(UiCommand::SetReplyModel {
        model: model.to_string(),
        usage,
        sources: Vec::new(),
    });
    app.process_command(UiCommand::Complete);

    let rows = render_rows(&mut app, width);
    let prompt = rows.iter().position(|r| r.contains("→ hi")).unwrap();
    assert!(!rows[prompt].contains(" · "), "{:?}", rows[prompt]);
    let reply = rows.iter().position(|r| r.contains("hello")).unwrap();
    // A blank separator row sits between the reply and its footer, so the
    // footer is the first row below the reply that carries a bracket.
    let footer = rows[reply + 1..]
        .iter()
        .find(|r| r.contains('['))
        .unwrap_or_else(|| panic!("no footer after reply: {rows:?}"));
    let start = footer.find('[').unwrap_or_else(|| panic!("{footer:?}"));
    let end = footer.find(']').unwrap();
    footer[start..=end].to_string()
}

#[test]
fn test_reply_footer_shows_model_tokens_and_cost_after_the_turn() {
    let usage = CallUsage {
        tokens: 12_400,
        cost_usd: Some(0.0418),
    };
    let footer = reply_footer_row("claude-opus-4-7", Some(usage), 200);
    assert_eq!(footer, "[claude-opus-4-7 · 12.4k tok · $0.042]");
}

#[test]
fn test_reply_footer_without_usage_or_cost() {
    assert_eq!(reply_footer_row("qwen3:4b", None, 200), "[qwen3:4b]");

    let local = CallUsage {
        tokens: 900,
        cost_usd: None,
    };
    assert_eq!(
        reply_footer_row("qwen3:4b", Some(local), 200),
        "[qwen3:4b · 900 tok]"
    );
}

#[test]
fn test_reply_footer_drops_cost_then_tokens_when_narrow() {
    use super::segments::{reply_footer, reply_meta};

    let usage = CallUsage {
        tokens: 1_250_000,
        cost_usd: Some(3.5),
    };
    let meta = reply_meta(Some("claude-opus-4-7"), Some(usage), &[]);
    let footer = |width| {
        reply_footer(&meta, width, &test_theme()).map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
    };
    assert_eq!(footer(50).unwrap(), "[claude-opus-4-7 · 1.2M tok · $3.50]");
    assert_eq!(footer(30).unwrap(), "[claude-opus-4-7 · 1.2M tok]");
    assert_eq!(footer(20).unwrap(), "[claude-opus-4-7]");
    assert_eq!(footer(12).unwrap(), "[claude-op…]");
    assert!(footer(3).is_none());
    assert!(reply_footer(&[], 50, &test_theme()).is_none());
}

#[test]
fn test_reply_meta_formats_tokens_and_cost() {
    use super::segments::reply_meta;

    let meta = |tokens, cost_usd| {
        reply_meta(Some("m"), Some(CallUsage { tokens, cost_usd }), &[]).join(" · ")
    };
    assert_eq!(meta(842, Some(0.004)), "m · 842 tok · <$0.01");
    assert_eq!(meta(12_449, Some(0.0418)), "m · 12.4k tok · $0.042");
    assert_eq!(meta(240_000, Some(1.234)), "m · 240k tok · $1.23");
    assert_eq!(meta(3_400_000, None), "m · 3.4M tok");
    assert_eq!(reply_meta(None, None, &[]), Vec::<String>::new());
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
    app.scroll_chat_to_top();
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
    let rows: Vec<&str> = live.lines().collect();
    let row = |needle: &str| rows.iter().position(|r| r.contains(needle));
    assert!(
        row("⬢ Thinking…") > row("Read lib.rs"),
        "the working line stays below the live steps: {live}"
    );

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
    let view = render_rows(&mut app, 120).join("\n");
    assert!(
        matches!(app.detail_view(), Some(crate::app::DetailView::File(_))),
        "a read opens in the file view: {view}"
    );
    assert!(view.contains("src/lib.rs"), "{view}");
    assert!(view.contains("1 │ fn main() {}"), "{view}");
}

/// Click the first chat text on screen that reads `needle`.
fn click_text(app: &mut crate::App, needle: &str) {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

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
}

fn read_tool(app: &mut crate::App, id: &str, path: &str) {
    use crate::events::{ToolData, UiCommand};

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

#[test]
fn test_turn_output_starts_right_under_the_prompt_while_steps_arrive() {
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    submit_message(&mut app, "fix it");
    let aura_row = |app: &mut crate::App| {
        let rows = render_rows(app, 120);
        let prompt = rows
            .iter()
            .position(|row| row.contains("→ fix it"))
            .unwrap();
        let below = &rows[prompt + 2];
        let text = below.trim_matches(|c: char| c.is_whitespace() || c == '│');
        assert!(!text.is_empty(), "{}", rows.join("\n"));
        assert!(!rows.join("\n").contains("AURA  "));
        prompt
    };
    let waiting = aura_row(&mut app);

    app.process_command(UiCommand::StartThinking);
    app.process_command(UiCommand::AppendThinking("Look at main.".to_string()));
    assert_eq!(aura_row(&mut app), waiting);
    app.process_command(UiCommand::FinishThinking);
    assert_eq!(aura_row(&mut app), waiting);
    let between = render_rows(&mut app, 120).join("\n");
    assert!(between.contains("⬢ Thinking…"), "{between}");

    read_tool(&mut app, "t1", "src/main.rs");
    assert_eq!(aura_row(&mut app), waiting);
    app.process_command(UiCommand::StartStreaming);
    assert_eq!(aura_row(&mut app), waiting);
    app.process_command(UiCommand::AppendText("Fixed.".to_string()));
    app.process_command(UiCommand::Complete);
    assert_eq!(aura_row(&mut app), waiting);
    let done = render_rows(&mut app, 120).join("\n");
    assert!(done.contains("Fixed."), "{done}");
}

#[test]
fn test_live_steps_are_numbered_in_order_then_condense() {
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    submit_message(&mut app, "look around");
    for i in 1..=6 {
        read_tool(&mut app, &format!("t{i}"), &format!("src/f{i}.rs"));
    }
    let live = render_rows(&mut app, 120).join("\n");
    assert!(live.contains("Exploring 6 files · step 6"), "{live}");
    for i in 1..=6 {
        assert!(live.contains(&format!("{i} ▸ Read f{i}.rs")), "{live}");
    }

    app.process_command(UiCommand::Complete);
    let done = render_rows(&mut app, 120).join("\n");
    assert!(done.contains("▸ Explored 6 files"), "{done}");
    assert!(done.contains("6 steps (click to expand)"), "{done}");
    assert!(!done.contains("Read f1.rs"), "{done}");

    click_text(&mut app, "6 steps (click to expand)");
    let open = render_rows(&mut app, 120).join("\n");
    assert!(open.contains("▾ Explored 6 files"), "{open}");
    assert!(open.contains("1 ▸ Read f1.rs"), "{open}");
    assert!(!open.contains("click to expand"), "{open}");
}

fn record_at(seq: u64, ts_ms: u64) -> crate::events::RecordSummary {
    crate::events::RecordSummary {
        seq,
        timestamp: "12:00:00".to_string(),
        full_hash: String::new(),
        hash_suffix: String::new(),
        tx_kind: "UserPrompt".to_string(),
        sender: String::new(),
        message: String::new(),
        action_count: 0,
        effect_status: String::new(),
        status: crate::events::RecordStatus::Ok,
        info: String::new(),
        what: String::new(),
        group: String::new(),
        detail: String::new(),
        error_details: String::new(),
        tx_id: String::new(),
        agent_id: String::new(),
        ts_ms,
        policy: None,
    }
}

#[test]
fn test_open_thinking_shows_its_reasoning_and_jumps_to_turn_records() {
    use crate::app::PanelFocus;
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    app.process_command(UiCommand::NewRecord(record_at(1, 1_000)));
    submit_message(&mut app, "fix it");
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
    app.process_command(UiCommand::NewRecord(record_at(2, now)));
    app.process_command(UiCommand::NewRecord(record_at(3, now + 5)));
    app.process_command(UiCommand::StartThinking);
    app.process_command(UiCommand::FinishThinking);
    read_tool(&mut app, "t1", "src/main.rs");
    app.process_command(UiCommand::Complete);

    click_text(&mut app, "click to expand");
    click_text(&mut app, "Thought");
    let open = render_rows(&mut app, 120).join("\n");
    assert!(
        open.contains("the model shared no reasoning text"),
        "{open}"
    );
    assert!(open.contains("→ records for this turn"), "{open}");

    click_text(&mut app, "records for this turn");
    assert_eq!(app.focus(), PanelFocus::Records);
    assert_eq!(app.selected_record(), 1);
}

#[test]
fn test_clicking_the_working_indicator_jumps_to_turn_records() {
    use crate::app::PanelFocus;
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    app.process_command(UiCommand::NewRecord(record_at(1, 1_000)));
    submit_message(&mut app, "hi");
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
    app.process_command(UiCommand::NewRecord(record_at(2, now)));

    click_text(&mut app, "Thinking…");
    assert_eq!(app.focus(), PanelFocus::Records);
    assert_eq!(app.selected_record(), 1);
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
    assert!(
        rows.contains("policy: write_file denied by user.off"),
        "{rows}"
    );
}

#[test]
fn test_policy_lines_follow_a_star_and_stack() {
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
    let star = rows[first].find("* policy: write_file").unwrap();
    let before = rows[first][..star].trim_end_matches(' ');
    assert!(
        before.ends_with('│') && before.len() + 2 <= star,
        "{}",
        rows[first]
    );
    assert_eq!(row("policy: edit_file allowed by user.on"), first + 1);
}

#[test]
fn test_a_scrolled_projects_list_draws_the_rows_it_shows() {
    use crate::app::ProjectListRow;
    use crate::events::{AgentSummary, UiCommand};
    use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};

    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let mut app = crate::App::new().with_event_sender(tx);
    let agent = |id: String, name: String, depth: usize| AgentSummary {
        zns_id: format!("0://terminal/{id}"),
        id,
        name,
        depth,
        ..AgentSummary::default()
    };
    let mut agents = Vec::new();
    for project in 0..12 {
        agents.push(agent(format!("p{project}"), format!("Lead{project:02}"), 0));
        for helper in 0..3 {
            agents.push(agent(
                format!("p{project}h{helper}"),
                format!("Helper{project:02}-{helper}"),
                1,
            ));
        }
    }
    app.process_command(UiCommand::SetAgents(agents));
    app.process_command(UiCommand::SetActiveAgent("p0".to_string()));

    let buffer = render_buffer(&mut app, 120);
    let top = row_of(&buffer, "LEAD00");
    let label = |app: &crate::App, row: usize| match app.project_rows()[row] {
        ProjectListRow::Project { root, .. } => Some(app.agents()[root].name.to_uppercase()),
        ProjectListRow::Agent { index } => Some(app.agents()[index].name.clone()),
        _ => None,
    };
    for _ in 0..30 {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 4,
            row: top,
            modifiers: KeyModifiers::empty(),
        });
        let buffer = render_buffer(&mut app, 120);
        let first = app.agents_scroll();
        for offset in 0..3 {
            let y = top + u16::try_from(offset).unwrap();
            if let Some(name) = label(&app, first + offset) {
                assert!(find_cell(&buffer, y, &name).is_some(), "{name} at {y}");
            }
        }
        assert!(row_of(&buffer, "New project") > top);
    }
    assert!(app.agents_scroll() > 0);
}

#[test]
fn test_smooth_scrolling_draws_the_chat_row_below_the_body() {
    use crate::events::UiCommand;

    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let mut app = crate::App::new().with_event_sender(tx);
    let lines: String = (0..80).map(|n| format!("line {n}\n\n")).collect();
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText(lines));
    render_buffer(&mut app, 120);
    assert_eq!(app.chat_scroll(), None, "only when the front end asks");

    app.set_smooth_scroll(true);
    // Up past the blank rows kept below the last line, to a line of text.
    let up = chat_room(&app) + 21;
    app.scroll_chat_rows(isize::try_from(up).unwrap());
    render_buffer(&mut app, 120);
    let view = app.chat_scroll().expect("the chat is drawn");
    assert_eq!(view.below, up);
    let (cells, _) = app.chat_row_below();
    let next: String = cells.iter().map(ratatui::buffer::Cell::symbol).collect();

    app.scroll_chat_rows(-1);
    let buffer = render_buffer(&mut app, 120);
    let view = app.chat_scroll().expect("the chat is drawn");
    assert_eq!(view.below, up - 1);
    let last = view.body.bottom() - 1;
    let row: String = (view.body.left()..view.body.right())
        .map(|x| buffer[(x, last)].symbol())
        .collect();
    let body = usize::from(view.body.x)..usize::from(view.body.right());
    assert_eq!(row, next[body].to_string(), "the row that scrolls in");
    assert!(row.contains("line"), "{row:?}");
}

#[test]
fn test_truncate_record_info_zero_width() {
    assert_eq!(truncate_record_info("anything", 0), "");
    assert_eq!(truncate_record_info("", 20), "");
}

fn mouse_at(
    app: &mut crate::App,
    kind: crossterm::event::MouseEventKind,
    (column, row): (u16, u16),
) {
    app.handle_mouse(crossterm::event::MouseEvent {
        kind,
        column,
        row,
        modifiers: crossterm::event::KeyModifiers::empty(),
    });
}

/// Turn the wheel up over the chat panel until it shows its top.
fn scroll_chat_to_top(app: &mut crate::App, on: &str) {
    let buffer = render_buffer(app, 120);
    let row = row_of(&buffer, on);
    for _ in 0..100 {
        mouse_at(app, crossterm::event::MouseEventKind::ScrollUp, (60, row));
    }
}

/// Press on `first` in the chat panel, hold the pointer below the screen
/// until the chat scrolls `last` into view, then let go. Checks that the
/// rows scrolled in are selected and that the copy holds `first` and
/// everything after it up to `last`, which it returns, a line per row.
fn select_past_the_bottom(app: &mut crate::App, first: &str, last: &str) -> Vec<String> {
    use crossterm::event::{MouseButton, MouseEventKind};

    let on_screen = |buffer: &ratatui::buffer::Buffer, needle| {
        (0..30).find(|&y| find_cell(buffer, y, needle).is_some())
    };
    let buffer = render_buffer(app, 120);
    let top = row_of(&buffer, first);
    let (column, _) = find_cell(&buffer, top, first).unwrap();
    assert!(on_screen(&buffer, last).is_none(), "{last:?} starts below");

    mouse_at(app, MouseEventKind::Down(MouseButton::Left), (column, top));
    let below = (119, 29);
    mouse_at(app, MouseEventKind::Drag(MouseButton::Left), below);
    assert!(app.selection_scrolling(), "held past the bottom");
    let mut buffer = render_buffer(app, 120);
    for _ in 0..40 {
        if on_screen(&buffer, last).is_some() {
            break;
        }
        app.scroll_selection();
        buffer = render_buffer(app, 120);
    }
    assert!(on_screen(&buffer, first).is_none(), "scrolled away");
    let shown = on_screen(&buffer, last).expect("scrolled into view");
    let (at, _) = find_cell(&buffer, shown, last).unwrap();
    assert!(
        buffer[(at, shown)]
            .modifier
            .contains(ratatui::style::Modifier::REVERSED),
        "the rows scrolled into view are selected"
    );
    mouse_at(app, MouseEventKind::Up(MouseButton::Left), below);
    assert!(!app.selection_scrolling());
    let buffer = render_buffer(app, 120);

    let text = app.take_copy_text(&buffer).expect("copied on release");
    let copied: Vec<String> = text
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    assert!(copied[0].contains(first), "{copied:?}");
    assert!(copied.iter().any(|l| l.contains(last)), "{copied:?}");
    copied
}

/// `line 00` to `line 40`, all in `copied`.
fn assert_copied_lines_to_40(copied: &[String]) {
    for n in 0..=40 {
        let line = format!("line {n:02}");
        assert!(
            copied.iter().any(|l| l.contains(&line)),
            "{line}: {copied:?}"
        );
    }
}

#[test]
fn test_a_selection_dragged_past_the_chat_scrolls_it_and_copies_it_all() {
    use crate::events::UiCommand;

    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let mut app = crate::App::new().with_event_sender(tx);
    let lines: String = (0..60).map(|n| format!("line {n:02}\n\n")).collect();
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText(lines));
    scroll_chat_to_top(&mut app, "line 59");

    let copied = select_past_the_bottom(&mut app, "line 00", "line 40");
    assert_eq!(copied[0], "line 00");
    assert_copied_lines_to_40(&copied);
}

#[test]
fn test_a_selection_dragged_past_the_terminal_scrolls_it() {
    use crate::events::UiCommand;

    let mut app = app_with_model(None);
    submit_message(&mut app, "$seq");
    let output: String = (0..60).map(|n| format!("line {n:02}\n")).collect();
    app.process_command(UiCommand::CommandFinished {
        output,
        exit_code: None,
    });
    app.process_command(UiCommand::Complete);
    assert_eq!(app.chat_tab(), crate::app::ChatTab::Terminal);
    scroll_chat_to_top(&mut app, "line 59");

    let copied = select_past_the_bottom(&mut app, "line 00", "line 40");
    assert_copied_lines_to_40(&copied);
}

#[test]
fn test_a_selection_dragged_past_a_benchmark_scrolls_it() {
    use crate::events::{BenchArm, BenchRun, BenchView, UiCommand};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = crate::App::new();
    app.process_command(UiCommand::BenchList {
        agent_id: String::new(),
        root: String::new(),
        benches: vec![BenchView {
            id: "ab".to_string(),
            name: "Compare #1".to_string(),
            arms: [BenchArm::default(), BenchArm::default()],
            runs: (0..60)
                .map(|n| BenchRun {
                    name: format!("line {n:02}"),
                    ..BenchRun::default()
                })
                .collect(),
            ..BenchView::default()
        }],
    });
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    assert_eq!(app.chat_tab(), crate::app::ChatTab::Bench);
    app.handle_key(KeyEvent::from(KeyCode::Enter));

    let copied = select_past_the_bottom(&mut app, "Compare #1", "line 40");
    assert_copied_lines_to_40(&copied);

    let buffer = render_buffer(&mut app, 120);
    let row = row_of(&buffer, "line 40");
    for _ in 0..3 {
        mouse_at(
            &mut app,
            crossterm::event::MouseEventKind::ScrollUp,
            (60, row),
        );
    }
    assert!(app.selection().is_some(), "the wheel keeps the selection");
}

#[test]
fn test_a_selection_dragged_past_a_record_scrolls_it() {
    use crate::events::{RecordStatus, RecordSummary, UiCommand};
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = crate::App::new();
    app.process_command(UiCommand::NewRecord(RecordSummary {
        seq: 7,
        timestamp: "12:00:00".to_string(),
        full_hash: String::new(),
        hash_suffix: String::new(),
        tx_kind: "Prompt".to_string(),
        sender: String::new(),
        message: (0..60).map(|n| format!("line {n:02}\n\n")).collect(),
        action_count: 0,
        effect_status: String::new(),
        status: RecordStatus::Ok,
        info: String::new(),
        what: String::new(),
        group: String::new(),
        detail: String::new(),
        error_details: String::new(),
        tx_id: String::new(),
        agent_id: String::new(),
        ts_ms: 0,
        policy: None,
    }));
    while app.focus() != crate::app::PanelFocus::Records {
        app.handle_key(KeyEvent::from(KeyCode::Tab));
    }
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.detail_view(), Some(&crate::app::DetailView::Record));

    let copied = select_past_the_bottom(&mut app, "Sequence:", "line 40");
    assert_copied_lines_to_40(&copied);
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
    let notes = |rows: Vec<String>| -> Vec<(usize, String)> {
        rows.into_iter()
            .enumerate()
            .filter_map(|(y, row)| row.find("note ").map(|i| (y, row[i..i + 7].to_string())))
            .collect()
    };
    let before = render_rows(&mut app, 100);
    assert!(before.join("\n").contains("lines below"));

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
    // The prompt sits under its frame's border and padding rows.
    assert!(row("latest prompt") <= row(" CHAT ") + 6, "{screen}");
    assert!(screen.contains("short reply"), "{screen}");
    assert!(!screen.contains("note 39"), "{screen}");
    assert!(!screen.contains("lines below"), "{screen}");

    app.scroll_up(3);
    let screen = render_rows(&mut app, 100).join("\n");
    assert!(!screen.contains("lines below"), "{screen}");
}

/// Blank rows the chat keeps below its last line, as it was last drawn.
fn chat_room(app: &crate::App) -> usize {
    let text = app.areas.chat_text.expect("the chat is drawn");
    super::panels::breathing_room(usize::from(text.height))
}

/// The row of `needle` in `rows`, and how many blank rows lie between it
/// and the chat's bottom border.
fn rows_under(rows: &[String], needle: &str) -> (usize, usize) {
    let screen = rows.join("\n");
    let row = rows
        .iter()
        .position(|row| row.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not in:\n{screen}"));
    let column = rows[row][..rows[row].find(needle).unwrap()].chars().count();
    let border = (row + 1..rows.len())
        .find(|&y| rows[y].chars().nth(column) == Some('─'))
        .unwrap_or_else(|| panic!("no border below {needle:?}:\n{screen}"));
    let blank = |row: &String| {
        row.chars()
            .skip(column)
            .take(needle.chars().count())
            .all(|c| c == ' ')
    };
    assert!(
        rows[row + 1..border].iter().all(blank),
        "only blank rows below {needle:?}:\n{screen}"
    );
    (row, border - row - 1)
}

#[test]
fn test_chat_keeps_room_below_a_reply_that_reaches_the_bottom() {
    use crate::components::{Message, MessageRole};
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    app.add_message(Message::new(MessageRole::User, "fix the login bug"));
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("line 00".to_string()));
    let rows = render_rows(&mut app, 100);
    let height = app.areas.chat_text.expect("the chat is drawn").height;
    let room = usize::from(height) / 4;
    assert!(room > 1, "a quarter of {height} rows");
    let (_, under) = rows_under(&rows, "line 00");
    assert!(under > room, "a short reply leaves the rest blank");

    let mut scrolled = false;
    for i in 1..40 {
        app.process_command(UiCommand::AppendText(format!("\n\nline {i:02}")));
        let rows = render_rows(&mut app, 100);
        let screen = rows.join("\n");
        let (_, under) = rows_under(&rows, &format!("line {i:02}"));
        assert!(
            under >= room,
            "line {i:02} too close to the bottom:\n{screen}"
        );
        assert!(!screen.contains("lines below"), "{screen}");
        if !screen.contains("fix the login bug") {
            // The reply pushed the prompt away: it scrolls with the room.
            assert_eq!(under, room, "{screen}");
            scrolled = true;
        }
    }
    assert!(scrolled, "the reply outgrew the chat");

    // The rows under the last line are blank, not lines left to read.
    app.scroll_up(room);
    let rows = render_rows(&mut app, 100);
    let screen = rows.join("\n");
    assert_eq!(rows_under(&rows, "line 39").1, 0, "{screen}");
    assert!(!screen.contains("lines below"), "{screen}");
    app.scroll_up(1);
    let screen = render_rows(&mut app, 100).join("\n");
    assert!(screen.contains(" ↓ 1 line below "), "{screen}");
}

#[test]
fn test_terminal_output_runs_to_the_bottom_row() {
    use crate::events::UiCommand;

    let mut app = app_with_model(None);
    submit_message(&mut app, "$seq");
    let output: String = (0..60).map(|n| format!("line {n:02}\n")).collect();
    app.process_command(UiCommand::CommandFinished {
        output,
        exit_code: None,
    });
    app.process_command(UiCommand::Complete);
    assert_eq!(app.chat_tab(), crate::app::ChatTab::Terminal);
    let rows = render_rows(&mut app, 120);
    assert_eq!(rows_under(&rows, "line 59").1, 0, "{}", rows.join("\n"));
}

#[test]
fn test_chat_runs_from_the_top_border_to_the_bottom_one() {
    use crate::components::{Message, MessageRole};

    let mut app = crate::App::new();
    app.add_message(Message::new(MessageRole::User, "prompt"));
    let reply: Vec<String> = (0..60).map(|i| format!("line {i:02}")).collect();
    app.add_message(Message::new(MessageRole::Assistant, &reply.join("\n\n")));
    render_rows(&mut app, 100);
    // Up past the blank rows kept below the last line.
    app.scroll_up(chat_room(&app));
    let rows = render_rows(&mut app, 100);
    let screen = rows.join("\n");
    let last = rows
        .iter()
        .position(|row| row.contains("line 59"))
        .unwrap_or_else(|| panic!("last line not in:\n{screen}"));
    let column = rows[last][..rows[last].find("line 59").unwrap()]
        .chars()
        .count();
    let border = |row: &String| row.chars().nth(column) == Some('─');
    assert!(
        border(&rows[last + 1]),
        "last line on the bottom row:\n{screen}"
    );
    assert!(!screen.contains("lines below"), "{screen}");

    app.scroll_chat_to_top();
    let rows = render_rows(&mut app, 100);
    let first = rows
        .iter()
        .position(|row| row.contains("prompt"))
        .expect("the prompt at the top");
    // Above the prompt: its frame's padding row, its top border, then the
    // panel's top border.
    assert!(rows[first - 2].contains('╭'), "prompt framed:\n{screen}");
    assert!(
        border(&rows[first - 3]),
        "first line on the top row:\n{screen}"
    );
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
fn test_prose_input_is_in_the_reply_color() {
    let theme = test_theme();
    let mut app = app_with_model(None);
    type_text(&mut app, "please explain");
    let buffer = render_buffer(&mut app, 120);

    let (_, style) = find_cell(&buffer, buffer.area.height - 3, "please").unwrap();
    assert_eq!(style.fg, Some(theme.colors.text));
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
    assert!(
        !rows[echo + 3].contains("[u@h d]$ "),
        "{:?}",
        rows[echo + 3]
    );
    let input_row = &rows[rows.len() - INPUT_FROM_BOTTOM];
    assert!(input_row.contains("[u@h d]$ "), "{input_row:?}");
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

/// The right border of the body of the panel whose header bar holds
/// `title`, top to bottom, without its corners.
fn right_border(rows: &[String], title: &str) -> String {
    let header = rows.iter().position(|r| r.contains(title)).unwrap();
    let start = rows[header][..rows[header].find(title).unwrap()]
        .chars()
        .count();
    let top = header + 2;
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

/// The chat panel's right border beside its text, below the header bar,
/// the body's top border and the blank row above the conversation, down
/// to the line above the input.
fn chat_track(rows: &[String]) -> String {
    let (tabs, _, right) = chat_panel(rows);
    rows[tabs + 4..]
        .iter()
        .map(|r| r.chars().nth(right).unwrap())
        .take_while(|&c| c != '┤')
        .collect()
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
    let rows = render_rows(&mut app, 250);
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
    let records = right_border(&rows, "│KRN│");
    assert!(
        records.starts_with('│') && records.ends_with('┃'),
        "{records:?}\n{screen}"
    );

    app.scroll_chat_to_top();
    let rows = render_rows(&mut app, 120);
    let chat = chat_track(&rows);
    assert!(chat.starts_with('┃') && chat.ends_with('│'), "{chat:?}");
}

fn run_slash(app: &mut crate::App, command: &str) {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    type_text(app, command);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
}

fn todo(content: &str, state: crate::events::TodoState) -> crate::events::TodoView {
    crate::events::TodoView {
        content: content.to_string(),
        state,
        assignee: None,
        task: None,
    }
}

#[test]
fn test_status_line_shows_context_cache_and_cost() {
    use crate::events::{CallStats, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::CallStats(CallStats {
        model: "aura-claude-sonnet-4-6".to_string(),
        input_tokens: 4_000,
        cache_read_tokens: 120_000,
        cache_write_tokens: 0,
        output_tokens: 900,
        cost_usd: Some(3.42),
        context_limit: Some(200_000),
    }));
    app.process_command(UiCommand::SetQueue(vec![crate::events::QueuedItemView {
        id: 1,
        text: "next".to_string(),
        editable: true,
    }]));

    let rows = render_rows(&mut app, 160);
    let status = &rows[rows.len() - 1];
    assert!(status.contains("62% context used"), "{status:?}");
    assert!(status.contains("96% cached"), "{status:?}");
    assert!(status.contains("$3.42"), "{status:?}");
    assert!(status.contains("1 call"), "{status:?}");
    assert!(status.contains("1 queued"), "{status:?}");
}

#[test]
fn test_status_line_drops_trailing_parts_when_narrow() {
    use crate::events::{CallStats, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::CallStats(CallStats {
        input_tokens: 10_000,
        context_limit: Some(200_000),
        cost_usd: Some(0.5),
        ..CallStats::default()
    }));
    let rows = render_rows(&mut app, 90);
    let status = &rows[rows.len() - 1];
    assert!(status.contains("5% context used"), "{status:?}");
    assert!(!status.contains("call"), "{status:?}");
}

#[test]
fn test_todo_checklist_and_list() {
    use crate::events::{TodoState, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::SetTodos(vec![
        todo("Read the parser", TodoState::Completed),
        todo("Write the lexer", TodoState::InProgress),
        todo("Run the tests", TodoState::Pending),
    ]));

    let screen = render_rows(&mut app, 160).join("\n");
    assert!(
        !screen.contains("Working on"),
        "only while a turn runs: {screen}"
    );
    assert!(!screen.contains("TODOS"));

    run_slash(&mut app, "/todos");
    let screen = render_rows(&mut app, 160).join("\n");
    assert!(screen.contains("TODOS 1/3"), "{screen}");
    assert!(screen.contains("☒ Read the parser"));
    assert!(screen.contains("☐ Run the tests"));
    run_slash(&mut app, "/todos");

    submit_message(&mut app, "go on");
    let rows = render_rows(&mut app, 160);
    let row = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not in:\n{}", rows.join("\n")))
    };
    let header = row("⬢ Working on 3 to-dos");
    assert_eq!(row("☒ Read the parser"), header + 1);
    assert_eq!(row("▣ Write the lexer"), header + 2);
    assert_eq!(row("☐ Run the tests"), header + 3);
    assert_eq!(row("⬢ Thinking…"), header + 5);
    // The boxes line up under the header's `⬢`.
    let column = |row: usize, glyph: char| {
        rows[row]
            .chars()
            .position(|c| c == glyph)
            .unwrap_or_else(|| panic!("no {glyph} in {:?}", rows[row]))
    };
    let hex = column(header, '⬢');
    assert_eq!(column(header + 1, '☒'), hex);
    assert_eq!(column(header + 2, '▣'), hex);
    assert_eq!(column(header + 3, '☐'), hex);

    app.process_command(UiCommand::SetTodos(Vec::new()));
    let screen = render_rows(&mut app, 160).join("\n");
    assert!(!screen.contains("Working on"));
}

#[test]
fn test_context_and_cost_commands_write_to_the_chat() {
    use crate::events::{CallStats, ContextView, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::CallStats(CallStats {
        input_tokens: 2_000,
        cache_read_tokens: 8_000,
        output_tokens: 500,
        cost_usd: Some(0.02),
        context_limit: Some(100_000),
        ..CallStats::default()
    }));
    app.process_command(UiCommand::SetContext(ContextView {
        system_tokens: 3_000,
        tools_tokens: 4_000,
        conversation_tokens: 3_000,
        total_tokens: 10_000,
        limit_tokens: Some(100_000),
        ..ContextView::default()
    }));

    run_slash(&mut app, "/context");
    run_slash(&mut app, "/cost");
    let texts: Vec<String> = app
        .messages()
        .iter()
        .map(|m| m.content().to_string())
        .collect();
    assert!(texts[0].contains("10.0k of 100k tokens (10%)"), "{texts:?}");
    assert!(texts[0].contains("tools"), "{texts:?}");
    assert!(texts[0].contains("Prompt cache hits: 80%"), "{texts:?}");
    assert!(texts[1].contains("$0.020"), "{texts:?}");
}

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
    run_slash(&mut app, "/todos");
    let screen = render_rows(&mut app, 160).join("\n");
    assert!(screen.contains("▣ Background work"), "{screen}");
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
    assert!(screen.contains("Approval Required"), "{screen}");
    assert!(screen.contains("run_command wants to:"), "{screen}");
    assert!(screen.contains("cargo test -p aura-terminal"), "{screen}");
    assert!(screen.contains("[Y] Allow once"), "{screen}");
    assert!(
        screen.contains("[A] Always allow `cargo test` commands in this project"),
        "{screen}"
    );
    assert!(screen.contains("[N] Deny (Esc)"), "{screen}");
}

#[test]
fn test_help_lists_mode_command_and_shift_tab() {
    let mut app = crate::App::new();
    run_slash(&mut app, "/help");
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 50)).unwrap();
    terminal
        .draw(|frame| super::render(frame, &mut app, &test_theme()))
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
    assert!(screen.contains("/mode [name]"), "{screen}");
    assert!(
        screen.contains("/model [opus] /thinking [level]  Model, thinking"),
        "fits the help box on one line: {screen}"
    );
    assert!(screen.contains("Shift+Tab"), "{screen}");
    assert!(screen.contains("Press any key to close"), "{screen}");
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
    run_slash(&mut app, "/context");
    run_slash(&mut app, "/new");
    run_slash(&mut app, "/context");

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
    use super::panels::spawn_guides;
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

#[test]
fn test_tasks_tab_lists_requests_with_their_parts() {
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
    let _ = app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    let rows = render_rows(&mut app, 200);
    let row_with = |text: &str| {
        rows.iter()
            .position(|r| r.contains(text))
            .unwrap_or_else(|| panic!("no {text} in:\n{}", rows.join("\n")))
    };
    let request = row_with("Build the compiler");
    assert!(
        rows[request].contains("Lead") && rows[request].contains("1/2"),
        "{}",
        rows[request]
    );
    assert!(
        rows[request + 1].contains("☒  Write the parser  → coder"),
        "{}",
        rows[request + 1]
    );
    assert!(
        rows[request + 2].contains("☐  Write the checker"),
        "{}",
        rows[request + 2]
    );
    // An item's box sits under its task's icon.
    let last_column = |row: usize, glyph: char| {
        rows[row]
            .chars()
            .enumerate()
            .filter_map(|(i, c)| (c == glyph).then_some(i))
            .last()
            .unwrap_or_else(|| panic!("no {glyph} in {:?}", rows[row]))
    };
    assert_eq!(last_column(request + 1, '☒'), last_column(request, '○'));
    assert_eq!(last_column(request + 2, '☐'), last_column(request, '○'));
    let part = row_with("Parse the source  → coder");
    assert!(
        part > request + 2,
        "the part sits under the request's items"
    );
    assert!(
        rows.iter().any(|r| r.contains("p plan")),
        "the hint names the plan key"
    );
}

// ========================================================================
// Folding agents over their spawns
// ========================================================================

/// Rows of the agents panel, its first 30 columns, for `rows`.
fn agents_panel(rows: &[String]) -> Vec<String> {
    rows.iter()
        .map(|row| row.chars().take(30).collect())
        .collect()
}

/// The panel row naming `name`, which must be listed once.
fn row_naming<'a>(panel: &'a [String], name: &str) -> &'a str {
    let found: Vec<&String> = panel.iter().filter(|row| row.contains(name)).collect();
    match found[..] {
        [row] => row,
        _ => panic!("{name:?} in {found:?}"),
    }
}

/// Whether `text` opens with the chat's braille spinner, which the
/// projects list shows beside a working agent, padded out to the icon column.
fn is_aura_ring(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| ('\u{2801}'..='\u{28ff}').contains(&c))
        && chars.all(|c| c == ' ')
}

fn spawn_tree_app(cc_running: bool) -> crate::App {
    use crate::events::{AgentSummary, UiCommand};
    let agent = |id: &str, name: &str, depth: usize| AgentSummary {
        id: id.to_string(),
        name: name.to_string(),
        depth,
        ..Default::default()
    };
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "Lead", 0),
        agent("bb", "planner", 1),
        AgentSummary {
            running: cc_running,
            ..agent("cc", "coder", 2)
        },
        agent("dd", "tester", 1),
    ]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app
}

#[test]
fn agents_with_spawns_show_a_fold_marker_and_leaves_do_not() {
    let mut app = spawn_tree_app(false);
    let panel = agents_panel(&render_rows(&mut app, 200));
    assert!(row_naming(&panel, "Lead").contains("▾"), "{panel:#?}");
    assert!(
        row_naming(&panel, "planner").contains("├▾  planner"),
        "{panel:#?}"
    );
    for leaf in ["coder", "tester"] {
        let row = row_naming(&panel, leaf);
        assert!(!row.contains('▾') && !row.contains('▸'), "{row:?}");
    }

    app.toggle_agent(1);
    let panel = agents_panel(&render_rows(&mut app, 200));
    assert!(
        row_naming(&panel, "planner").contains("├▸  planner"),
        "{panel:#?}"
    );
    assert!(!panel.iter().any(|row| row.contains("coder")), "{panel:#?}");
    assert!(row_naming(&panel, "tester").contains("└──"), "{panel:#?}");
}

/// The `AURA_WIDTH` cells after the elbow on `row`, and the cell after them.
fn ring_and_marker(row: &str) -> (String, Option<char>) {
    let at = row.find(['├', '└']).expect("elbow") + '├'.len_utf8();
    let mut cells = row[at..].chars();
    let ring = cells.by_ref().take(crate::animation::AURA_WIDTH).collect();
    (ring, cells.next())
}

#[test]
fn a_thinking_agent_with_spawns_shows_the_aura_without_a_marker() {
    let mut app = spawn_tree_app(false);
    let bb = app.agents()[1].clone();
    let mut agents = app.agents().to_vec();
    agents[1] = crate::events::AgentSummary {
        running: true,
        ..bb
    };
    app.process_command(crate::events::UiCommand::SetAgents(agents));
    let panel = agents_panel(&render_rows(&mut app, 200));
    let planner = row_naming(&panel, "planner");
    let (ring, marker) = ring_and_marker(planner);
    assert!(is_aura_ring(&ring), "{planner:?}");
    assert_eq!(marker, Some(' '), "{planner:?}");
    assert!(
        !planner.contains('▾') && !planner.contains('▸'),
        "{planner:?}"
    );
    assert!(
        planner.contains(" planner"),
        "names stay lined up: {planner:?}"
    );
}

#[test]
fn a_folded_agent_shows_the_aura_while_a_hidden_spawn_thinks() {
    let mut app = spawn_tree_app(true);
    let panel = agents_panel(&render_rows(&mut app, 200));
    assert!(
        row_naming(&panel, "planner").contains("├▾  planner"),
        "open, its spawn shows its own ring: {panel:#?}"
    );

    app.toggle_agent(1);
    let panel = agents_panel(&render_rows(&mut app, 200));
    let planner = row_naming(&panel, "planner");
    let (ring, marker) = ring_and_marker(planner);
    assert!(is_aura_ring(&ring), "{planner:?}");
    assert_ne!(marker, Some('▸'), "no arrow beside the aura: {planner:?}");
    assert!(!planner.contains('▾'), "{planner:?}");

    // Folding the top agent too carries the aura up to it.
    app.toggle_agent(0);
    let panel = agents_panel(&render_rows(&mut app, 200));
    let lead = row_naming(&panel, "Lead");
    assert!(!lead.contains('▸') && !lead.contains('▾'), "{lead:?}");
    assert!(
        lead.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)),
        "{lead:?}"
    );
}

#[test]
fn unfocused_the_folded_agent_over_the_active_one_is_lit() {
    let mut app = spawn_tree_app(false);
    app.process_command(crate::events::UiCommand::SetActiveAgent("cc".to_string()));
    app.toggle_agent(1);
    assert_ne!(app.focus(), crate::app::PanelFocus::Swarm);
    let buffer = render_buffer(&mut app, 120);
    let selection = Some(test_theme().colors.selection);
    let bg = |name: &str| {
        let (_, style) = find_cell(&buffer, row_of(&buffer, name), name).unwrap();
        style.bg
    };
    assert_eq!(bg("planner"), selection, "planner stands for coder");
    assert_ne!(bg("tester"), selection);
    assert_ne!(bg("Lead"), selection);
}

#[test]
fn clicking_copy_log_copies_the_log_and_says_so() {
    use crate::events::{LogRunView, LogView, UiCommand};
    use crossterm::event::{MouseButton, MouseEventKind};

    let mut app = crate::App::new();
    app.select_pane_tab(crate::PaneTab::Chat(crate::ChatTab::Log));
    app.process_command(UiCommand::LogView(Box::new(LogView {
        path: "/runs/r/log.json".into(),
        json: "{\"whole\":true}".into(),
        runs: vec![LogRunView {
            run_id: "r".into(),
            title: "fix it".into(),
            ..LogRunView::default()
        }],
        ..LogView::default()
    })));
    let buffer = render_buffer(&mut app, 120);
    let row = row_of(&buffer, "Copy log");
    let (column, _) = find_cell(&buffer, row, "Copy log").unwrap();
    mouse_at(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        (column, row),
    );
    mouse_at(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        (column, row),
    );
    assert!(app.copy_pending(), "the click asked for a copy");
    let buffer = render_buffer(&mut app, 120);
    assert_eq!(
        app.take_copy_text(&buffer).as_deref(),
        Some("{\"whole\":true}")
    );
    app.notify_copied(14, &Ok(()));
    assert!(app
        .notification()
        .is_some_and(|(message, _)| message.contains("Copied the log")));
    let buffer = render_buffer(&mut app, 120);
    assert!(
        find_cell(&buffer, row, "✓ Copied").is_some(),
        "the button says so"
    );
}

// ========================================================================
// Plan mode badge
// ========================================================================

/// `Lead`, on screen and in plan mode, and `Researcher`, in neither.
fn plan_mode_app() -> crate::App {
    use crate::events::{AgentSummary, UiCommand};
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![
        AgentSummary {
            id: "aa".to_string(),
            name: "Lead".to_string(),
            plan_mode: true,
            ..AgentSummary::default()
        },
        AgentSummary {
            id: "bb".to_string(),
            name: "Researcher".to_string(),
            ..AgentSummary::default()
        },
    ]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app
}

/// `mode` for agent `agent_id`, as the session sends a mode switch.
fn set_mode_for(agent_id: &str, mode: crate::PermissionMode) -> crate::events::UiCommand {
    crate::events::UiCommand::ForAgent {
        agent_id: agent_id.to_string(),
        command: Box::new(crate::events::UiCommand::SetMode(mode)),
    }
}

/// Asserts the three cells of `(P)` on row `y`, starting at `x`, are drawn
/// in the accent color on `bg`.
fn assert_plan_badge_cells(buffer: &ratatui::buffer::Buffer, (x, y): (u16, u16), bg: Color) {
    let colors = test_theme().colors;
    for cx in x..x + 3 {
        let style = buffer[(cx, y)].style();
        assert_eq!(style.fg, Some(colors.success), "column {cx}");
        assert_eq!(style.bg, Some(bg), "column {cx}");
    }
}

#[test]
fn an_agent_in_plan_mode_shows_the_plan_badge_in_the_accent_color() {
    let mut app = plan_mode_app();
    let colors = test_theme().colors;
    assert_ne!(
        colors.success, colors.on_selection,
        "the check below needs it"
    );

    // Lead is on screen, so its row is the selected one: the badge keeps
    // its color on the selection, unlike the name.
    let buffer = render_buffer(&mut app, 120);
    let row = row_of(&buffer, "Lead (P)");
    let (name_at, name) = find_cell(&buffer, row, "Lead").unwrap();
    assert_eq!(name.bg, Some(colors.selection), "Lead is the selected row");
    assert_eq!(name.fg, Some(colors.on_selection));
    let (badge_at, _) = find_cell(&buffer, row, "(P)").unwrap();
    assert_eq!(badge_at, name_at + 5, "` (P)` right after the name");
    assert_plan_badge_cells(&buffer, (badge_at, row), colors.selection);
    assert_eq!(
        buffer[(badge_at - 1, row)].style().bg,
        Some(colors.selection),
        "the badge's space sits on the selection too"
    );

    // On screen elsewhere, its row is plain and the badge still accented.
    app.process_command(crate::events::UiCommand::SetActiveAgent("bb".to_string()));
    let buffer = render_buffer(&mut app, 120);
    let row = row_of(&buffer, "Lead (P)");
    let (_, name) = find_cell(&buffer, row, "Lead").unwrap();
    assert_eq!(name.bg, Some(Color::Reset), "Lead is not selected");
    let (badge_at, _) = find_cell(&buffer, row, "(P)").unwrap();
    assert_plan_badge_cells(&buffer, (badge_at, row), Color::Reset);
}

#[test]
fn an_agent_out_of_plan_mode_shows_no_plan_badge() {
    let mut app = plan_mode_app();
    let panel = agents_panel(&render_rows(&mut app, 120));
    assert!(
        row_naming(&panel, "Lead").contains("Lead (P)"),
        "{panel:#?}"
    );
    let researcher = row_naming(&panel, "Researcher");
    assert!(!researcher.contains("(P)"), "{researcher:?}");
    assert_eq!(
        panel.iter().filter(|row| row.contains("(P)")).count(),
        1,
        "{panel:#?}"
    );
}

#[test]
fn the_plan_badge_follows_mode_switches_live() {
    use crate::PermissionMode;
    let mut app = plan_mode_app();

    // Researcher, off screen, goes into plan mode; Lead, on screen, out.
    app.process_command(set_mode_for("bb", PermissionMode::Plan));
    app.process_command(set_mode_for("aa", PermissionMode::Agent));
    let panel = agents_panel(&render_rows(&mut app, 120));
    assert!(
        row_naming(&panel, "Researcher").contains("Researcher (P)"),
        "{panel:#?}"
    );
    assert!(!row_naming(&panel, "Lead").contains("(P)"), "{panel:#?}");

    let buffer = render_buffer(&mut app, 120);
    let row = row_of(&buffer, "Researcher (P)");
    let (badge_at, _) = find_cell(&buffer, row, "(P)").unwrap();
    assert_plan_badge_cells(&buffer, (badge_at, row), Color::Reset);

    app.process_command(set_mode_for("bb", PermissionMode::ReadOnly));
    let panel = agents_panel(&render_rows(&mut app, 120));
    assert!(panel.iter().all(|row| !row.contains("(P)")), "{panel:#?}");
}

#[test]
fn a_long_name_in_plan_mode_is_cut_and_keeps_its_plan_badge() {
    use crate::events::{AgentSummary, UiCommand};
    let name = "An agent with a very long name that never ends";
    let three_minutes_ago = super::panels::now_ms() - 3 * 60 * 1000;
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![AgentSummary {
        id: "aa".to_string(),
        name: name.to_string(),
        plan_mode: true,
        last_active_ms: Some(three_minutes_ago),
        ..AgentSummary::default()
    }]));
    // Off screen, so the row is not selected and its cells are plain.
    app.process_command(UiCommand::SetActiveAgent("zz".to_string()));

    let rows = render_rows(&mut app, 120);
    let row_at = |rows: &[String]| {
        rows.iter()
            .position(|row| row.contains("An agent"))
            .unwrap_or_else(|| panic!("no agent row in:\n{}", rows.join("\n")))
    };
    let at = row_at(&rows);
    let row = &rows[at];
    assert!(!row.contains(name), "the name is cut: {row:?}");
    assert!(row.contains("… (P) 3m │"), "{row:?}");
    // The cut name, `…` included, and the badge fill the room the whole
    // name has without it: the row is as wide as any other in the panel.
    let right_border = |row: &str| row.chars().skip(2).position(|c| c == '│');
    assert_eq!(right_border(row), right_border(&rows[at - 1]), "{rows:#?}");
    let shown_name = |row: &str| {
        let start = row.find("An agent").unwrap();
        let end = row.find('…').unwrap();
        row[start..end].chars().count()
    };
    let with_badge = shown_name(row);

    let buffer = render_buffer(&mut app, 120);
    let y = u16::try_from(at).unwrap();
    let (badge_at, _) = find_cell(&buffer, y, "(P)").unwrap();
    assert_plan_badge_cells(&buffer, (badge_at, y), Color::Reset);
    assert_eq!(buffer[(badge_at - 2, y)].symbol(), "…");

    // Out of plan mode the name takes the badge's columns back.
    app.process_command(set_mode_for("aa", crate::PermissionMode::Agent));
    let rows = render_rows(&mut app, 120);
    let row = &rows[row_at(&rows)];
    assert!(!row.contains("(P)"), "{row:?}");
    assert!(row.contains("… 3m │"), "{row:?}");
    assert_eq!(shown_name(row), with_badge + PLAN_BADGE_WIDTH, "{row:?}");
    assert_eq!(right_border(row), right_border(&rows[at - 1]));
}

/// Columns of [`super::panels::PLAN_BADGE`].
const PLAN_BADGE_WIDTH: usize = 4;

#[test]
fn a_tree_row_keeps_its_width_with_the_plan_badge_and_drops_it_when_too_narrow() {
    use super::panels::{cell_style, tree_row, CellState, RowBadge, StatusIcon, PLAN_BADGE};
    let theme = test_theme();
    assert_eq!(display_width(PLAN_BADGE), PLAN_BADGE_WIDTH);
    let badge = RowBadge {
        text: PLAN_BADGE,
        color: theme.colors.success,
    };
    let row = |width: usize, badge: Option<RowBadge<'_>>| {
        let cell = cell_style(&theme, CellState::Normal, Style::default());
        let icon = StatusIcon::new("", theme.colors.muted);
        tree_row(
            width,
            ("", theme.colors.muted),
            icon,
            ("Researcher", badge),
            cell,
            &crate::app::AgentRowEnd::time(Some("3m")),
            &[],
        )
    };
    let text = |line: &ratatui::text::Line<'_>| -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    };
    // ` `, the icon column and its pad, then the name, then ` 3m `.
    let around = 1 + crate::animation::AURA_WIDTH + 1 + 4;
    for width in around + 1..=around + 20 {
        let plain = row(width, None);
        let marked = row(width, Some(badge));
        assert_eq!(plain.width(), width, "width {width}");
        assert_eq!(marked.width(), width, "width {width}");
        let name_room = width - around;
        let marked_text = text(&marked);
        if name_room >= PLAN_BADGE_WIDTH + 2 {
            let badge_span = marked
                .spans
                .iter()
                .find(|span| span.content == PLAN_BADGE)
                .unwrap_or_else(|| panic!("no badge at width {width}: {marked_text:?}"));
            assert_eq!(badge_span.style.fg, Some(theme.colors.success));
            let name = marked_text.trim_start().split(PLAN_BADGE).next().unwrap();
            assert!(!name.is_empty(), "width {width}: {marked_text:?}");
            if name_room - PLAN_BADGE_WIDTH < "Researcher".len() {
                assert!(name.ends_with('…'), "width {width}: {marked_text:?}");
            } else {
                assert_eq!(name, "Researcher", "width {width}");
            }
        } else {
            assert!(
                !marked_text.contains("(P)"),
                "fewer than 2 name columns drop the badge, width {width}: {marked_text:?}"
            );
            assert_eq!(marked_text, text(&plain), "width {width}");
        }
    }
}

/// Agents `aa` ("Terminal Agent", active three minutes ago) and `bb`
/// ("Researcher", no time), with codes `aa_code` and `bb_code`.
fn app_with_coded_agents(aa_code: Option<&str>, bb_code: Option<&str>) -> crate::App {
    use crate::events::{AgentSummary, UiCommand};
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![
        AgentSummary {
            id: "aa".to_string(),
            name: "Terminal Agent".to_string(),
            last_active_ms: Some(super::now_ms() - 3 * 60 * 1000),
            code: aa_code.map(str::to_string),
            ..Default::default()
        },
        AgentSummary {
            id: "bb".to_string(),
            name: "Researcher".to_string(),
            code: bb_code.map(str::to_string),
            ..Default::default()
        },
    ]));
    app
}

/// Index of the first of `rows` that contains `needle`.
fn line_with(rows: &[String], needle: &str) -> usize {
    rows.iter()
        .position(|row| row.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not in:\n{}", rows.join("\n")))
}

/// Column of `needle` in `row`, counted in characters.
fn char_column(row: &str, needle: &str) -> usize {
    let i = row
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
    row[..i].chars().count()
}

#[test]
fn agent_rows_end_in_their_codes_lined_up_at_the_right_edge() {
    use crate::events::UiCommand;
    let mut app = app_with_coded_agents(Some("RE36"), Some("AG1"));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));

    let rows = render_rows(&mut app, 120);
    let aa = line_with(&rows, "Terminal Ag");
    let bb = line_with(&rows, "Researcher");
    assert!(rows[aa].contains(" 3m RE36 │"), "{:?}", rows[aa]);
    // The shorter code is padded, so both start in the same column.
    assert!(rows[bb].contains(" AG1  │"), "{:?}", rows[bb]);
    assert_eq!(
        char_column(&rows[aa], "RE36"),
        char_column(&rows[bb], "AG1")
    );

    // Muted like the time, on the selected row's fill.
    let colors = test_theme().colors;
    let buffer = render_buffer(&mut app, 120);
    let (aa, bb) = (u16::try_from(aa).unwrap(), u16::try_from(bb).unwrap());
    let (_, code) = find_cell(&buffer, aa, "RE36").expect("code");
    let (_, time) = find_cell(&buffer, aa, "3m").expect("time");
    let (_, name) = find_cell(&buffer, aa, "Terminal").expect("name");
    assert_eq!(code.fg, time.fg);
    assert_eq!(code.bg, name.bg);
    assert!(name.bg.is_some_and(|bg| bg != Color::Reset), "{name:?}");
    let (_, code) = find_cell(&buffer, bb, "AG1").expect("code");
    assert_eq!(code.fg, Some(colors.muted));
}

#[test]
fn hovered_agent_row_draws_its_button_where_the_hit_test_puts_it_beside_a_code() {
    use crate::app::{agent_button_offset, AgentButton, AgentRowEnd, HoverTarget};
    use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
    let moved = |column: usize, row: usize| MouseEvent {
        kind: MouseEventKind::Moved,
        column: u16::try_from(column).unwrap(),
        row: u16::try_from(row).unwrap(),
        modifiers: KeyModifiers::empty(),
    };
    let mut app = app_with_coded_agents(Some("RE36"), Some("AG1"));

    let before = render_rows(&mut app, 120);
    let line = line_with(&before, "Terminal Ag");
    let row: Vec<char> = before[line].chars().collect();
    let name = char_column(&before[line], "Terminal Ag");
    let left = row[..name].iter().rposition(|&c| c == '│').expect("border");
    let edge = name + row[name..].iter().position(|&c| c == '│').expect("border");
    let list_x = left + 1;
    let width = edge - list_x;
    let end = AgentRowEnd::agent(width, 0, Some("3m"), (Some("RE36"), 4));
    let offset = |button| {
        let width = u16::try_from(width).unwrap();
        list_x + usize::from(agent_button_offset(width, &end, button))
    };

    app.handle_mouse(moved(name, line));
    let hovered = render_rows(&mut app, 120);
    assert!(
        hovered[line].contains(" ✕ 3m RE36 │"),
        "{:?}",
        hovered[line]
    );
    let glyph_at = |row: &str, column: usize| row.chars().nth(column);
    assert_eq!(
        glyph_at(&hovered[line], offset(AgentButton::Archive)),
        Some('✕')
    );
    assert_eq!(
        char_column(&hovered[line], " 3m RE36 "),
        char_column(&before[line], " 3m RE36 "),
        "the time and code stay put on hover"
    );

    app.handle_mouse(moved(offset(AgentButton::Archive), line));
    assert!(matches!(app.hover(), Some(HoverTarget::ArchiveAgent(_))));
    app.handle_mouse(moved(offset(AgentButton::Archive) - 2, line));
    assert!(matches!(app.hover(), Some(HoverTarget::ProjectRow(_))));
    app.handle_mouse(moved(char_column(&before[line], "RE36"), line));
    assert!(matches!(app.hover(), Some(HoverTarget::ProjectRow(_))));
}

#[test]
fn a_tree_row_drops_its_code_when_the_name_gets_too_narrow_and_still_fits() {
    use super::panels::{cell_style, tree_row, CellState, StatusIcon};
    use crate::app::{AgentButton, AgentRowEnd};
    let theme = test_theme();
    let text = |line: &ratatui::text::Line<'_>| -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    };
    let buttons = AgentButton::ALL.map(|button| (button, theme.colors.muted));
    // ` `, the icon column and its pad, ` ✕`, then ` 3m RE36 `.
    let around = 1 + crate::animation::AURA_WIDTH + 1 + 2 + 9;
    for width in around..=around + 10 {
        let end = AgentRowEnd::agent(width, 0, Some("3m"), (Some("RE36"), 4));
        for buttons in [&buttons[..], &[]] {
            let cell = cell_style(&theme, CellState::Normal, Style::default());
            let icon = StatusIcon::new("", theme.colors.muted);
            let line = tree_row(
                width,
                ("", theme.colors.muted),
                icon,
                ("Researcher", None),
                cell,
                &end,
                buttons,
            );
            let line_text = text(&line);
            assert_eq!(line.width(), width, "width {width}: {line_text:?}");
            if width >= around + 4 {
                assert!(line_text.ends_with(" 3m RE36 "), "{line_text:?}");
            } else {
                assert!(!line_text.contains("RE36"), "{line_text:?}");
                assert!(line_text.ends_with(" 3m "), "{line_text:?}");
            }
        }
    }
}

#[test]
fn agent_rows_without_codes_are_laid_out_as_before() {
    use super::panels::{cell_style, tree_row, CellState, StatusIcon};
    use crate::animation::AURA_WIDTH;
    use crate::app::{agent_code_width, AgentButton, AgentRowEnd};
    let theme = test_theme();
    let text = |line: &ratatui::text::Line<'_>| -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    };
    let buttons = AgentButton::ALL.map(|button| (button, theme.colors.muted));
    let row = |ago: Option<&str>, buttons: &[(AgentButton, Color)]| {
        let cell = cell_style(&theme, CellState::Normal, Style::default());
        let icon = StatusIcon::new("", theme.colors.muted);
        let end = AgentRowEnd::agent(24, 0, ago, (None, 0));
        text(&tree_row(
            24,
            ("", theme.colors.muted),
            icon,
            ("Researcher", None),
            cell,
            &end,
            buttons,
        ))
    };
    let lead = format!(" {}", " ".repeat(AURA_WIDTH + 1));
    assert_eq!(
        row(Some("3m"), &[]),
        format!("{lead}{:<16} 3m ", "Researcher")
    );
    assert_eq!(row(None, &[]), format!("{lead}{:<20}", "Researcher"));
    assert_eq!(
        row(Some("3m"), &buttons),
        format!("{lead}{:<14} ✕ 3m ", "Researcher")
    );
    assert_eq!(
        row(None, &buttons),
        format!("{lead}{:<17} ✕ ", "Researcher")
    );

    // In the panel, a list with no codes has no code column.
    let mut app = app_with_coded_agents(None, None);
    assert_eq!(agent_code_width(app.agents()), 0);
    let rows = render_rows(&mut app, 120);
    let aa = &rows[line_with(&rows, "Terminal Ag")];
    assert!(aa.contains(" 3m │"), "{aa:?}");
    let bb = &rows[line_with(&rows, "Researcher")];
    let after_name = &bb[bb.find("Researcher").unwrap() + "Researcher".len()..];
    let pad = after_name.find('│').expect("border");
    assert!(after_name[..pad].chars().all(|c| c == ' '), "{bb:?}");
}

/// `/model` on an app holding `count` local models (all ready) and one
/// Claude plan model that needs `/login claude`, drawn `height` rows tall.
fn model_selector_app(count: usize) -> crate::App {
    use crate::events::{ModelOptionView, UiCommand};

    let model = |id: String, label: String, group: &str, provider: Option<&str>, available| {
        ModelOptionView {
            id,
            label,
            local: provider.is_none(),
            installed: true,
            group: group.to_string(),
            provider: provider.map(str::to_string),
            available,
            context_window: None,
        }
    };
    let mut options: Vec<ModelOptionView> = (0..count)
        .map(|i| {
            model(
                format!("local/m{i:02}"),
                format!("Model {i:02}"),
                "Local",
                None,
                true,
            )
        })
        .collect();
    options.push(model(
        "claude/claude-sonnet-4-6".to_string(),
        "Claude Sonnet 4.6".to_string(),
        "Claude plan",
        Some("claude"),
        false,
    ));
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetModels {
        options,
        current: "local/m00".to_string(),
        route: None,
        user: None,
        plans: Vec::new(),
        thinking: None,
    });
    type_text(&mut app, "/model");
    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::empty(),
    ));
    app
}

fn render_rows_tall(app: &mut crate::App, width: u16, height: u16) -> Vec<String> {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
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
fn test_model_selector_shows_tabs_and_counts() {
    let mut app = model_selector_app(3);
    let screen = render_rows_tall(&mut app, 100, 30).join("\n");
    assert!(screen.contains("Select model"), "{screen}");
    assert!(screen.contains("Ready (3)"), "{screen}");
    assert!(screen.contains("Sign in (1)"), "{screen}");
    assert!(screen.contains("Type to filter"), "{screen}");
    assert!(screen.contains("● Model 00"), "{screen}");
    assert!(
        !screen.contains("Claude Sonnet 4.6"),
        "the signed-out model waits on the other tab: {screen}"
    );
}

#[test]
fn test_model_selector_scrolls_to_keep_the_cursor_in_view() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = model_selector_app(60);
    let screen = render_rows_tall(&mut app, 100, 20).join("\n");
    assert!(screen.contains("Model 00"), "{screen}");
    assert!(screen.contains("↓ more"), "{screen}");
    assert!(!screen.contains("Model 59"), "{screen}");

    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::empty()));
    let screen = render_rows_tall(&mut app, 100, 20).join("\n");
    assert!(screen.contains("›   Model 59"), "{screen}");
    assert!(screen.contains("↑ more"), "{screen}");
    assert!(!screen.contains("│    Model 00"), "{screen}");
    assert!(
        !screen.contains("↓ more"),
        "nothing is hidden below: {screen}"
    );
}

#[test]
fn test_model_selector_sign_in_tab_shows_the_login_to_run() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = model_selector_app(3);
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
    let screen = render_rows_tall(&mut app, 100, 30).join("\n");
    assert!(
        screen.contains("›   Claude Sonnet 4.6  /login claude"),
        "{screen}"
    );
    assert!(!screen.contains("│    Model 00"), "{screen}");
}

#[test]
fn test_model_selector_filters_as_you_type() {
    let mut app = model_selector_app(12);
    type_text(&mut app, "l 1");
    let screen = render_rows_tall(&mut app, 100, 30).join("\n");
    assert!(screen.contains("/ l 1█"), "{screen}");
    assert!(
        screen.contains("Model 10") && screen.contains("Model 11"),
        "{screen}"
    );
    assert!(!screen.contains("│    Model 02"), "{screen}");

    type_text(&mut app, "zz");
    let screen = render_rows_tall(&mut app, 100, 30).join("\n");
    assert!(screen.contains("No models match"), "{screen}");
}

#[test]
fn test_reply_footer_says_who_paid_for_the_tokens() {
    use crate::events::{SourceUsage, UiCommand};

    let mut app = crate::App::new();
    submit_message(&mut app, "hi");
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("hello".to_string()));
    app.process_command(UiCommand::SetReplyModel {
        model: "claude-sonnet-4-6 · claude-haiku-4.5".to_string(),
        usage: Some(CallUsage {
            tokens: 27_900,
            cost_usd: Some(0.004),
        }),
        sources: vec![
            SourceUsage {
                label: "Claude plan".to_string(),
                tokens: 25_100,
                cost_usd: None,
            },
            SourceUsage {
                label: "OpenRouter".to_string(),
                tokens: 2_800,
                cost_usd: Some(0.004),
            },
        ],
    });
    app.process_command(UiCommand::Complete);

    let rows = render_rows(&mut app, 140);
    let footer = rows
        .iter()
        .position(|r| r.contains("[claude-sonnet-4-6 · claude-haiku-4.5"))
        .expect("the model footer");
    assert!(
        !rows[footer].contains("27.9k tok"),
        "the sign-ins' shares replace the totals: {:?}",
        &rows[footer]
    );
    assert!(
        rows.get(footer + 1).is_none_or(|r| !r.contains('[')),
        "the footer is one line: {:?}",
        &rows[footer..]
    );
    let sources = [
        SourceUsage {
            label: "Claude plan".to_string(),
            tokens: 25_100,
            cost_usd: None,
        },
        SourceUsage {
            label: "OpenRouter".to_string(),
            tokens: 2_800,
            cost_usd: Some(0.004),
        },
    ];
    let usage = CallUsage {
        tokens: 27_900,
        cost_usd: Some(0.004),
    };
    assert_eq!(
        super::segments::reply_meta(Some("m"), Some(usage), &sources).join(" · "),
        "m · via Claude plan 25.1k tok · OpenRouter 2.8k tok <$0.01"
    );
}

#[test]
fn test_reply_footer_names_a_lone_sign_in_without_repeating_its_totals() {
    use super::segments::reply_meta;
    use crate::events::SourceUsage;

    let meta = reply_meta(
        Some("claude-opus-5-5 ×28 · high thinking"),
        Some(CallUsage {
            tokens: 1_600_000,
            cost_usd: Some(0.998),
        }),
        &[SourceUsage {
            label: "Claude plan".to_string(),
            tokens: 1_600_000,
            cost_usd: Some(0.998),
        }],
    );
    assert_eq!(
        meta.join(" · "),
        "claude-opus-5-5 ×28 · high thinking · 1.6M tok · $0.998 · via Claude plan"
    );
}
