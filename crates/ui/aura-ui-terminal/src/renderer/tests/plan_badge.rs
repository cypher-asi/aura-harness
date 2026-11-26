use super::*;

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
    let three_minutes_ago = aura_core_types::time::now_ms() - 3 * 60 * 1000;
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

/// Columns of [`crate::components::PLAN_BADGE`].
const PLAN_BADGE_WIDTH: usize = 4;

#[test]
fn a_tree_row_keeps_its_width_with_the_plan_badge_and_drops_it_when_too_narrow() {
    use crate::components::{cell_style, tree_row, CellState, RowBadge, StatusIcon, PLAN_BADGE};
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
            assert_badge_after_name(&marked, &theme, width, name_room);
        } else {
            assert!(
                !marked_text.contains("(P)"),
                "fewer than 2 name columns drop the badge, width {width}: {marked_text:?}"
            );
            assert_eq!(marked_text, text(&plain), "width {width}");
        }
    }
}

fn text(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// A row with room for the badge shows it, in the success color, after the
/// (maybe shortened) name.
fn assert_badge_after_name(
    marked: &ratatui::text::Line<'_>,
    theme: &Theme,
    width: usize,
    name_room: usize,
) {
    use crate::components::PLAN_BADGE;

    let marked_text = text(marked);
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
}
