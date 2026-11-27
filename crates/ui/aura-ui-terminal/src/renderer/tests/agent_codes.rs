use super::*;

/// Agents `aa` ("Terminal Agent", active three minutes ago) and `bb`
/// ("Researcher", no time), with codes `aa_code` and `bb_code`.
fn app_with_coded_agents(aa_code: Option<&str>, bb_code: Option<&str>) -> crate::App {
    use crate::events::{AgentSummary, UiCommand};
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![
        AgentSummary {
            id: "aa".to_string(),
            name: "Terminal Agent".to_string(),
            last_active_ms: Some(aura_core_types::time::now_ms() - 3 * 60 * 1000),
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

#[test]
fn no_agent_row_shows_a_code_at_the_right_edge() {
    use crate::events::UiCommand;
    let mut app = app_with_coded_agents(Some("REN36"), Some("AGE1"));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));

    let rows = render_rows(&mut app, 120);
    let aa = line_with(&rows, "Terminal Ag");
    let bb = line_with(&rows, "Researcher");
    // Neither the root agent nor its spawn shows its code; the time alone
    // ends the row and both names line up.
    assert!(rows[aa].contains(" 3m │"), "{:?}", rows[aa]);
    assert!(!rows[aa].contains("REN36"), "{:?}", rows[aa]);
    assert!(!rows[bb].contains("AGE1"), "{:?}", rows[bb]);
    // The time is the last text before the panel's right border; the
    // agent's code used to sit between them.
    let row = &rows[aa];
    let time_end = row.find(" 3m ").unwrap() + 4;
    let after_time = &row[time_end..];
    assert!(
        after_time.trim_start_matches(' ').starts_with('│'),
        "nothing between the time and the border: {row:?}"
    );
}

#[test]
fn hovered_agent_row_draws_its_button_where_the_hit_test_puts_it_beside_the_time() {
    use crate::app::{AgentButton, AgentRowEnd, HoverTarget};
    use crate::components::agent_button_offset;
    use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
    let moved = |column: usize, row: usize| MouseEvent {
        kind: MouseEventKind::Moved,
        column: u16::try_from(column).unwrap(),
        row: u16::try_from(row).unwrap(),
        modifiers: KeyModifiers::empty(),
    };
    let mut app = app_with_coded_agents(Some("REN36"), Some("AGE1"));

    let before = render_rows(&mut app, 120);
    let line = line_with(&before, "Terminal Ag");
    let row: Vec<char> = before[line].chars().collect();
    let name = char_column(&before[line], "Terminal Ag");
    let left = row[..name].iter().rposition(|&c| c == '│').expect("border");
    let edge = name + row[name..].iter().position(|&c| c == '│').expect("border");
    let list_x = left + 1;
    let width = edge - list_x;
    let end = AgentRowEnd::agent(Some("3m"));
    let offset = |button| {
        let width = u16::try_from(width).unwrap();
        list_x + usize::from(agent_button_offset(width, &end, button))
    };

    app.handle_mouse(moved(name, line));
    let hovered = render_rows(&mut app, 120);
    assert!(hovered[line].contains(" ✕ 3m │"), "{:?}", hovered[line]);
    let glyph_at = |row: &str, column: usize| row.chars().nth(column);
    assert_eq!(
        glyph_at(&hovered[line], offset(AgentButton::Archive)),
        Some('✕')
    );
    assert_eq!(
        char_column(&hovered[line], " 3m "),
        char_column(&before[line], " 3m "),
        "the time stays put on hover"
    );

    app.handle_mouse(moved(offset(AgentButton::Archive), line));
    assert!(matches!(app.hover(), Some(HoverTarget::ArchiveAgent(_))));
    app.handle_mouse(moved(offset(AgentButton::Archive) - 2, line));
    assert!(matches!(app.hover(), Some(HoverTarget::ProjectRow(_))));
    app.handle_mouse(moved(char_column(&before[line], "3m"), line));
    assert!(matches!(app.hover(), Some(HoverTarget::ProjectRow(_))));
}

#[test]
fn a_tree_row_keeps_its_whole_width_at_narrow_widths() {
    use crate::app::{AgentButton, AgentRowEnd};
    use crate::components::{cell_style, tree_row, CellState, StatusIcon};
    let theme = test_theme();
    let text = |line: &ratatui::text::Line<'_>| -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    };
    let buttons = AgentButton::ALL.map(|button| (button, theme.colors.muted));
    // ` `, the icon column and its pad, ` ✕`, then ` 3m ` is 10 columns.
    // Below that the fixed parts alone are wider than the row, which no
    // row layout cuts.
    let around = 1 + crate::animation::AURA_WIDTH + 1 + 2 + 4;
    for width in around..=around + 10 {
        let end = AgentRowEnd::agent(Some("3m"));
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
            assert!(line_text.ends_with(" 3m "), "{line_text:?}");
            assert_eq!(
                line.width(),
                width,
                "width {width}, buttons {}: {line_text:?}",
                buttons.len()
            );
        }
    }
}

#[test]
fn agent_rows_without_codes_are_laid_out_as_before() {
    use crate::animation::AURA_WIDTH;
    use crate::app::{AgentButton, AgentRowEnd};
    use crate::components::{cell_style, tree_row, CellState, StatusIcon};
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
        let end = AgentRowEnd::agent(ago);
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

    // In the panel, a coded agent gets the same rows as one without.
    let mut app = app_with_coded_agents(Some("REN36"), Some("AGE1"));
    let rows = render_rows(&mut app, 120);
    let aa = &rows[line_with(&rows, "Terminal Ag")];
    assert!(aa.contains(" 3m │"), "{aa:?}");
    let bb = &rows[line_with(&rows, "Researcher")];
    let after_name = &bb[bb.find("Researcher").unwrap() + "Researcher".len()..];
    let pad = after_name.find('│').expect("border");
    assert!(after_name[..pad].chars().all(|c| c == ' '), "{bb:?}");
}
