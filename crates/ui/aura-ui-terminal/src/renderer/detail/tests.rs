use super::*;

fn text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn hard_wrap_keeps_indentation_and_indents_continuations() {
    let rows = hard_wrap(Line::from("    let x = 1234567;"), 10, "  ");
    let rows: Vec<String> = rows.iter().map(text).collect();
    assert_eq!(rows, ["    let x ", "  = 123456", "  7;"]);
}

#[test]
fn short_line_is_untouched() {
    let rows = hard_wrap(Line::from("  ok"), 10, "");
    assert_eq!(rows.len(), 1);
    assert_eq!(text(&rows[0]), "  ok");
}

#[test]
fn sanitize_expands_tabs_and_masks_controls() {
    assert_eq!(sanitize("\tx\u{1b}"), "    x\u{fffd}");
}

fn screen(app: &mut App) -> Vec<Vec<String>> {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, app, &Theme::cyber()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

/// Corners of the chat panel's body, the box right below the header
/// bar holding its CHAT | TERMINAL tabs.
fn chat_corners(rows: &[Vec<String>]) -> ((usize, usize), (usize, usize)) {
    let (top, left) = rows
        .iter()
        .enumerate()
        .find_map(|(y, row)| {
            let text = row.concat();
            if ![" TERMINAL ", "│TERMINAL│", " TRM "]
                .iter()
                .any(|tab| text.contains(tab))
            {
                return None;
            }
            let chat =
                (0..row.len().saturating_sub(4)).find(|&x| row[x..x + 4].concat() == "CHAT")?;
            let x = (0..chat).rev().find(|&x| row[x] == "│")?;
            Some((y + 2, x))
        })
        .expect("no chat header bar");
    assert_eq!(rows[top][left], "┌");
    let right = (left + 1..rows[top].len())
        .find(|&x| rows[top][x] == "┐")
        .unwrap();
    let bottom = (top + 1..rows.len())
        .find(|&y| rows[y][left] == "└")
        .unwrap();
    ((top, left), (bottom, right))
}

#[test]
fn record_view_covers_exactly_the_chat_body() {
    use crate::events::{RecordStatus, RecordSummary, UiCommand};
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = App::new();
    app.process_command(UiCommand::NewRecord(RecordSummary {
        seq: 60,
        timestamp: "06:16:19".to_string(),
        full_hash: "355d52f6".repeat(8),
        hash_suffix: String::new(),
        tx_kind: "Policy".to_string(),
        sender: "KERNEL".to_string(),
        message: "approved".to_string(),
        action_count: 0,
        effect_status: "-".to_string(),
        status: RecordStatus::Ok,
        info: String::new(),
        what: String::new(),
        group: String::new(),
        detail: String::new(),
        error_details: String::new(),
        tx_id: "a71e37b3".repeat(8),
        agent_id: String::new(),
        ts_ms: 0,
        policy: None,
    }));
    let chat = chat_corners(&screen(&mut app));

    while app.focus() != crate::app::PanelFocus::Records {
        app.handle_key(KeyEvent::from(KeyCode::Tab));
    }
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    let rows = screen(&mut app);
    let text = rows
        .iter()
        .map(|r| r.concat())
        .collect::<Vec<_>>()
        .join("\n");
    let ((top, left), (bottom, right)) = chat;
    let corners = [(top, left), (top, right), (bottom, left), (bottom, right)]
        .map(|(y, x)| rows[y][x].as_str());
    assert_eq!(corners, ["┌", "┐", "└", "┘"], "{text}");
    let header: String = rows[top - 2][left..=right].concat();
    assert!(header.starts_with("│ RECORD #60 "), "{header}");
    assert!(header.ends_with(" ✓ POLICY ⤢ ✕ │"), "{header}");
    assert!(!text.contains("TERMINAL"), "the tabs give way: {text}");
    let border: String = rows[top][left..=right].concat();
    assert!(border.trim_matches(['┌', '─', '┐']).is_empty(), "{border}");
    assert!(text.contains("  Sequence      60"), "{text}");
    assert!(text.contains("Esc close"), "{text}");

    app.handle_key(KeyEvent::from(KeyCode::Esc));
    let rows = screen(&mut app);
    assert_eq!(chat_corners(&rows), chat, "closing brings the tabs back");

    app.handle_key(KeyEvent::from(KeyCode::Enter));
    screen(&mut app);
    let at = |column: usize| {
        move |kind| crossterm::event::MouseEvent {
            kind,
            column: u16::try_from(column).unwrap(),
            row: u16::try_from(top - 2).unwrap(),
            modifiers: crossterm::event::KeyModifiers::empty(),
        }
    };
    let expand = at(right - 4);
    app.handle_mouse(expand(crossterm::event::MouseEventKind::Moved));
    assert_eq!(app.hover(), Some(crate::app::HoverTarget::ExpandChat));
    app.handle_mouse(expand(crossterm::event::MouseEventKind::Down(
        crossterm::event::MouseButton::Left,
    )));
    assert!(app.chat_expanded(), "the ⤢ expands the chat");
    assert!(app.detail_view().is_some(), "with the record still on it");
    let rows = screen(&mut app);
    let header = &rows[top - 2];
    let wide_left = header.iter().position(|cell| cell == "│").unwrap();
    let wide_right = header.iter().rposition(|cell| cell == "│").unwrap();
    assert!(wide_left < left && wide_right > right, "the chat widens");
    let header: String = header[wide_left..=wide_right].concat();
    assert!(header.starts_with("│ RECORD #60 "), "{header}");
    assert!(header.ends_with(" ✓ POLICY ⤡ ✕ │"), "{header}");
    app.handle_mouse(at(wide_right - 4)(crossterm::event::MouseEventKind::Down(
        crossterm::event::MouseButton::Left,
    )));
    assert!(!app.chat_expanded(), "the ⤡ restores it");
    screen(&mut app);

    let close = at(right - 2);
    app.handle_mouse(close(crossterm::event::MouseEventKind::Moved));
    assert_eq!(app.hover(), Some(crate::app::HoverTarget::CloseDetail));
    app.handle_mouse(close(crossterm::event::MouseEventKind::Down(
        crossterm::event::MouseButton::Left,
    )));
    assert!(app.detail_view().is_none(), "the ✕ closes the record");
    let rows = screen(&mut app);
    assert_eq!(chat_corners(&rows), chat);
}

#[test]
fn the_named_header_bar_has_no_tabs_to_click() {
    use crate::app::ChatTab;
    use crate::events::UiCommand;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.md"), "# Notes\n").unwrap();
    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    let rows = screen(&mut app);
    let (row, column) = rows
        .iter()
        .enumerate()
        .find_map(|(y, row)| {
            let x =
                (0..row.len().saturating_sub(8)).find(|&x| row[x..x + 8].concat() == "TERMINAL")?;
            Some((y, x))
        })
        .expect("no TERMINAL tab");

    while app.focus() != crate::app::PanelFocus::Sandbox {
        app.handle_key(KeyEvent::from(KeyCode::Tab));
    }
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    screen(&mut app);
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: u16::try_from(column).unwrap(),
        row: u16::try_from(row).unwrap(),
        modifiers: KeyModifiers::empty(),
    });
    assert_eq!(app.chat_tab(), ChatTab::Chat);
    assert!(matches!(app.detail_view(), Some(DetailView::File(_))));
}

#[test]
fn file_view_numbers_lines_and_scrolls() {
    use crate::events::UiCommand;
    use crossterm::event::{KeyCode, KeyEvent};

    let dir = tempfile::tempdir().unwrap();
    let body: String = (1..=100).map(|n| format!("let x{n} = {n};\n")).collect();
    std::fs::write(dir.path().join("main.rs"), body).unwrap();
    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    while app.focus() != crate::app::PanelFocus::Sandbox {
        app.handle_key(KeyEvent::from(KeyCode::Tab));
    }
    app.handle_key(KeyEvent::from(KeyCode::Enter));

    let text = |app: &mut App| {
        screen(app)
            .iter()
            .map(|r| r.concat())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let first = text(&mut app);
    let header = first
        .lines()
        .find(|row| row.contains("│ main.rs "))
        .unwrap_or_else(|| panic!("no file header: {first}"));
    assert!(header.contains(" 100 LINES ⤢ ✕ │"), "{header}");
    assert!(!first.contains("TERMINAL"), "{first}");
    assert!(first.contains("  1 │ let x1 = 1;"), "{first}");

    app.handle_key(KeyEvent::from(KeyCode::Char('G')));
    let last = text(&mut app);
    assert!(last.contains("100 │ let x100 = 100;"), "{last}");
    assert!(last.contains("Ln 100/100, Col 1"), "{last}");
    assert!(!last.contains("  1 │ "), "{last}");

    for key in ":50\n".chars() {
        let code = if key == '\n' {
            KeyCode::Enter
        } else {
            KeyCode::Char(key)
        };
        app.handle_key(KeyEvent::from(code));
    }
    let middle = text(&mut app);
    assert!(middle.contains("Ln 50/100"), "{middle}");
    assert!(middle.contains(" 50 │ let x50 = 50;"), "{middle}");
}

#[test]
fn cursor_and_matches_restyle_cells() {
    let mut vim = Vim::default();
    let mut scroll = 0;
    let screen = crate::app::vim::Screen { first: 0, shown: 5 };
    let lines = ["\tab ab"];
    for c in "/ab\n".chars() {
        let code = match c {
            '\n' => crossterm::event::KeyCode::Enter,
            c => crossterm::event::KeyCode::Char(c),
        };
        vim.handle_key(code.into(), &lines, &mut scroll, screen);
    }
    assert_eq!((vim.line(), vim.col()), (0, 1));
    let theme = Theme::cyber();
    let spans = mark(vec![Span::raw("    ab ab")], lines[0], &vim, true, &theme);
    let pieces: Vec<(&str, bool)> = spans
        .iter()
        .map(|s| {
            let reversed = s.style.add_modifier.contains(Modifier::REVERSED);
            (s.content.as_ref(), reversed)
        })
        .collect();
    assert_eq!(
        pieces,
        [
            ("    ", false),
            ("a", true),
            ("b", false),
            (" ", false),
            ("ab", false)
        ]
    );
    assert_eq!(spans[2].style.bg, Some(theme.colors.selection));
    assert_eq!(spans[3].style.bg, None);
}

#[test]
fn fit_follows_the_cursor_and_centers_long_jumps() {
    let lines: Vec<String> = (0..100).map(|n| n.to_string()).collect();
    let source = Source::Code {
        language: String::new(),
        lines: lines.iter().map(String::as_str).collect(),
        gutter: 3,
        first_line: 1,
        highlighted: Highlighted::default(),
    };
    assert_eq!(fit(&source, Align::Cursor, 5, 0, 40, 10), 0);
    assert_eq!(fit(&source, Align::Cursor, 10, 0, 40, 10), 1);
    assert_eq!(fit(&source, Align::Cursor, 50, 0, 40, 10), 46);
    assert_eq!(fit(&source, Align::Cursor, 49, 50, 40, 10), 49);
    assert_eq!(fit(&source, Align::Top, 30, 0, 40, 10), 30);
    assert_eq!(fit(&source, Align::Bottom, 30, 0, 40, 10), 21);
    assert_eq!(fit(&source, Align::View, 30, 7, 40, 10), 7);
}

#[test]
fn last_page_counts_wrapped_rows() {
    let source = Source::Styled(vec![
        Line::from("a"),
        Line::from("b"),
        Line::from("cccccccccc"),
    ]);
    assert_eq!(last_page_start(&source, 5, 2), 2);
    assert_eq!(last_page_start(&source, 5, 3), 1);
    assert_eq!(last_page_start(&source, 5, 4), 0);
    assert_eq!(last_page_start(&source, 5, 1), 2);
}
