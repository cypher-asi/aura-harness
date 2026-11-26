use super::*;

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

/// Two agents: one active three minutes ago, one with no time.
fn archive_button_app() -> crate::App {
    use crate::events::{AgentSummary, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![
        AgentSummary {
            id: "aa".to_string(),
            name: "Terminal Agent".to_string(),
            last_active_ms: Some(aura_core_types::time::now_ms() - 3 * 60 * 1000),
            ..Default::default()
        },
        AgentSummary {
            id: "bb".to_string(),
            name: "Researcher".to_string(),
            ..Default::default()
        },
    ]));
    app
}

fn char_col(row: &str, needle: &str) -> usize {
    let i = row
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
    row[..i].chars().count()
}

fn glyph_at(row: &str, column: usize) -> Option<char> {
    row.chars().nth(column)
}

fn moved(column: usize, row: usize) -> crossterm::event::MouseEvent {
    crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Moved,
        column: u16::try_from(column).unwrap(),
        row: u16::try_from(row).unwrap(),
        modifiers: crossterm::event::KeyModifiers::empty(),
    }
}

/// An agent with no time has its archive button one column in from the
/// edge; returns that agent's row.
fn check_archive_button_with_no_time(app: &mut crate::App, before: &[String]) -> usize {
    use crate::app::HoverTarget;

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
    let buffer = render_buffer(app, 120);
    let (_, archive) = find_cell(&buffer, row, "✕").expect("archive button");
    assert_eq!(archive.fg, Some(colors.muted));
    app.handle_mouse(moved(edge - 2, other));
    assert!(matches!(app.hover(), Some(HoverTarget::ArchiveAgent(_))));
    let no_time = render_rows(app, 120);
    assert_eq!(glyph_at(&no_time[other], edge - 2), Some('✕'));
    let buffer = render_buffer(app, 120);
    let (_, archive) = find_cell(&buffer, row, "✕").expect("archive button");
    assert_eq!(archive.fg, Some(colors.foreground));
    other
}

#[test]
fn test_hovered_agent_row_shows_an_archive_button_left_of_its_time() {
    use crate::app::HoverTarget;

    let mut app = archive_button_app();
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

    let other = check_archive_button_with_no_time(&mut app, &before);

    // Off the rows, they look as they did.
    app.handle_mouse(moved(60, 0));
    let after = render_rows(&mut app, 120);
    assert_eq!(after[line], before[line]);
    assert_eq!(after[other], before[other]);
}

#[test]
fn test_header_hamburger_hides_and_restores_the_right_column() {
    use crate::app::{HoverTarget, PanelFocus};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let mut app = app_with_model(None);
    app.panels.sandbox_visible = false;
    app.focus = PanelFocus::Records;
    let rows = render_rows(&mut app, 120);
    assert!(rows[0].trim_end().ends_with('☰'), "{:?}", rows[0]);
    assert!(!rows[0].contains("http://"), "{:?}", rows[0]);
    let cell = app.side_toggle_area().expect("drawn");
    let mouse = |kind| MouseEvent {
        kind,
        column: cell.x + 1,
        row: cell.y,
        modifiers: KeyModifiers::empty(),
    };
    app.handle_mouse(mouse(MouseEventKind::Moved));
    assert_eq!(app.hover(), Some(HoverTarget::SideToggle));

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left)));
    assert!(!app.side_column_shown());
    assert!(!app.record_panel_visible());
    assert_eq!(app.focus(), PanelFocus::Chat, "off the hidden panel");
    let hidden = render_rows(&mut app, 120);
    assert!(
        !hidden.iter().any(|row| row.contains("No records yet")),
        "the Record panel is gone"
    );
    assert_eq!(app.side_toggle_area(), Some(cell), "the button stays put");

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left)));
    assert!(app.record_panel_visible(), "the Record panel comes back");
    assert!(!app.sandbox_panel_visible(), "only what was shown");
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
    use crossterm::event::{MouseButton, MouseEventKind};

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
    mouse_at(&mut app, MouseEventKind::Moved, (expand, row));
    assert_eq!(app.hover(), Some(HoverTarget::ExpandSide));
    let buffer = render_buffer(&mut app, 120);
    let lit = buffer[(expand, row)].style();
    assert_eq!(lit.bg, Some(test_theme().colors.surface_hover));
    let click = MouseEventKind::Down(MouseButton::Left);
    mouse_at(&mut app, click, (expand, row));
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
    mouse_at(&mut app, click, (restore, row));
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
