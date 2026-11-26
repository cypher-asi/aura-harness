use super::*;

#[test]
fn clicking_a_finished_boot_consoles_fold_opens_and_closes_it() {
    use crate::components::BootConsole;
    use crate::events::{BootLogLine, BootStageState, BootUpdate};

    let mut console = BootConsole::new("boot-1", "Moving to the cloud");
    console.apply(BootUpdate::Stage {
        key: "create".into(),
        label: "Creating machine".into(),
        state: BootStageState::Done { ms: 1200 },
    });
    console.apply(BootUpdate::Log(vec![BootLogLine {
        stderr: false,
        text: "booted line".into(),
    }]));
    console.apply(BootUpdate::End {
        ok: true,
        summary: "Now running in the cloud.".into(),
    });
    let mut app = App::new();
    app.add_message(Message::boot(console));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    draw(&mut app, &mut terminal);
    assert!(cell_of(&terminal, "booted line").is_none(), "folded");

    let hint = cell_of(&terminal, "click to show the boot log").expect("the hint is on screen");
    click_at(&mut app, hint);
    assert!(app.messages()[0].is_expanded(), "the hint opens it");
    draw(&mut app, &mut terminal);
    assert!(cell_of(&terminal, "booted line").is_some());
    assert!(cell_of(&terminal, "Creating machine").is_some());

    let summary = cell_of(&terminal, "Now running in the cloud.").expect("summary on screen");
    click_at(&mut app, summary);
    assert!(!app.messages()[0].is_expanded(), "the outcome closes it");
    draw(&mut app, &mut terminal);

    let header = cell_of(&terminal, "Moving to the cloud").expect("header on screen");
    click_at(&mut app, header);
    assert!(app.messages()[0].is_expanded(), "the header opens it");
}

#[test]
fn clicking_the_more_lines_row_of_a_tool_result_shows_the_rest() {
    let output: Vec<String> = (0..250).map(|i| format!("result line {i}")).collect();
    let mut app = App::new();
    app.add_message(Message::activity(
        Activity::Tool(ToolStep {
            id: "t1".to_string(),
            name: "read_file".to_string(),
            args: r#"{"path":"big.txt"}"#.to_string(),
            status: ToolStatus::Success,
            result: Some(output.join("\n")),
            retry: None,
            started: None,
            duration_ms: Some(10),
            diff: None,
            policy: None,
            awaiting_approval: false,
            live: crate::components::LiveOutput::default(),
        }),
        true,
    ));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    draw(&mut app, &mut terminal);
    assert!(cell_of(&terminal, "result line 249").is_none());
    let row = cell_of(&terminal, "… 50 more lines").expect("the more lines row is on screen");

    click_at(&mut app, row);
    draw(&mut app, &mut terminal);
    assert!(cell_of(&terminal, "result line 249").is_some());
    assert!(cell_of(&terminal, "more lines").is_none());
}

#[test]
fn test_dragging_the_chat_scrollbar_scrolls_without_selecting() {
    use crossterm::event::MouseButton::Left;
    use crossterm::event::MouseEventKind::{Down, Drag, Up};

    let (mut app, mut terminal) = overflowing_chat();
    draw(&mut app, &mut terminal);
    let bar = app.areas.scrollbar.expect("the chat overflows");
    assert_eq!(bar.target, scrollbar::ScrollTarget::Chat);
    let chat = app.areas.chat.expect("the chat is drawn");
    assert_eq!(bar.track.x, chat.right() - 1, "on the right border");
    assert_eq!(app.scroll_offset(), 0, "at the bottom");

    // The thumb sits at the bottom; take it there and drag it to the top.
    let (column, bottom) = (bar.track.x, bar.track.bottom() - 1);
    let press = |app: &mut App, kind, row| app.handle_mouse(mouse(kind, column, row));
    press(&mut app, Down(Left), bottom);
    assert!(app.selection().is_none(), "no text selection starts");
    assert!(app.select.selecting.is_none());
    assert_eq!(app.scroll_offset(), 0, "pressing the thumb leaves it");

    let moved = press(&mut app, Drag(Left), bar.track.y);
    assert!(moved, "the chat scrolled, so there is a frame to draw");
    assert_eq!(app.scroll_offset(), bar.total - bar.visible, "at the top");
    draw(&mut app, &mut terminal);
    assert_eq!(app.areas.scrollbar.map(|bar| bar.first), Some(0));

    // Halfway down the track is halfway through the conversation.
    let middle = bar.track.y + bar.track.height / 2;
    press(&mut app, Drag(Left), middle);
    let first = (bar.total - bar.visible) - app.scroll_offset();
    let half = bar.total / 2;
    assert!(
        first.abs_diff(half) <= bar.total / 4,
        "{first} of {}",
        bar.total
    );
    press(&mut app, Up(Left), middle);
    assert!(app.selection().is_none());

    // Released, a drag over the chat is the selection's again.
    let offset = app.scroll_offset();
    press(&mut app, Drag(Left), bottom);
    assert_eq!(app.scroll_offset(), offset);

    // A press on the track away from the thumb jumps it there.
    draw(&mut app, &mut terminal);
    press(&mut app, Down(Left), bottom);
    assert_eq!(app.scroll_offset(), 0, "back at the bottom");
    press(&mut app, Up(Left), bottom);
    assert!(app.selection().is_none());
}

#[test]
fn test_the_blank_columns_left_of_the_chat_scrollbar_grab_it_too() {
    use crossterm::event::MouseButton::Left;
    use crossterm::event::MouseEventKind::{Down, Drag, Up};

    let (mut app, mut terminal) = overflowing_chat();
    draw(&mut app, &mut terminal);
    let bar = app.areas.scrollbar.expect("the chat overflows");
    let rows = app.areas.chat_rows.expect("the chat's rows");
    assert!(
        bar.track.x - rows.right() > 2,
        "more blank columns than it reaches: {rows:?} beside {bar:?}"
    );
    let (top, bottom) = (bar.track.y, bar.track.bottom() - 1);
    let farthest = bar.track.x - 2;
    let press = |app: &mut App, kind, column, row| app.handle_mouse(mouse(kind, column, row));

    // Two columns in, a press at the top jumps the thumb there and a
    // drag takes it back down.
    press(&mut app, Down(Left), farthest, top);
    assert!(app.scrollbar_held());
    assert!(app.selection().is_none(), "no text selection starts");
    assert_eq!(app.scroll_offset(), bar.last, "at the top");
    press(&mut app, Drag(Left), farthest, bottom);
    assert_eq!(app.scroll_offset(), 0, "back at the bottom");
    press(&mut app, Up(Left), farthest, bottom);
    assert!(!app.scrollbar_held());

    // A column further in is the chat's, blank or not.
    draw(&mut app, &mut terminal);
    let inside = farthest - 1;
    press(&mut app, Down(Left), inside, top);
    assert!(!app.scrollbar_held());
    assert_eq!(app.scroll_offset(), 0, "the chat stayed put");
    press(&mut app, Up(Left), inside, top);
}

#[test]
fn test_a_chat_that_fits_has_no_scrollbar_to_grab() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let mut app = App::new();
    app.add_message(Message::new(MessageRole::Assistant, "short"));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    draw(&mut app, &mut terminal);
    assert!(app.areas.scrollbar.is_none());
    let chat = app.areas.chat.expect("the chat is drawn");
    let rows = app.areas.chat_rows.expect("the chat's rows");
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        chat.right() - 1,
        rows.y + 1,
    ));
    assert!(!app.scrollbar_held());
}

#[test]
fn test_the_thumb_at_the_bottom_of_a_long_chat_shows_its_end_and_follows_it() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let mut app = App::new();
    for i in 0..1000 {
        app.add_message(Message::new(MessageRole::Assistant, &format!("reply {i}")));
    }
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    draw(&mut app, &mut terminal);
    let bar = app.areas.scrollbar.expect("the chat overflows");
    // Long enough that the track's last row stands for more rows than
    // fit on screen.
    let track = usize::from(bar.track.height);
    assert!(bar.total > bar.visible * track, "{bar:?}");
    let (column, top, bottom) = (bar.track.x, bar.track.y, bar.track.bottom() - 1);
    let press = |app: &mut App, kind, row| app.handle_mouse(mouse(kind, column, row));

    // Taken at the bottom, dragged to the top and back down.
    press(&mut app, MouseEventKind::Down(MouseButton::Left), bottom);
    press(&mut app, MouseEventKind::Drag(MouseButton::Left), top);
    assert_eq!(app.scroll_offset(), bar.last, "at the top");
    draw(&mut app, &mut terminal);
    press(&mut app, MouseEventKind::Drag(MouseButton::Left), bottom);
    assert_eq!(app.scroll_offset(), 0, "at the very bottom");
    press(&mut app, MouseEventKind::Up(MouseButton::Left), bottom);
    app.add_message(Message::new(MessageRole::Assistant, "the latest"));
    draw(&mut app, &mut terminal);
    assert_eq!(app.scroll_offset(), 0, "following what comes in");

    // A few rows up the thumb is still drawn on the last row. Pressed
    // there it stays put; moved along that row it goes to the end.
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, column - 5, top + 2));
    draw(&mut app, &mut terminal);
    let offset = app.scroll_offset();
    assert!(offset > 0);
    press(&mut app, MouseEventKind::Down(MouseButton::Left), bottom);
    assert_eq!(app.scroll_offset(), offset, "pressing the thumb leaves it");
    app.handle_mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        column - 1,
        bottom,
    ));
    assert_eq!(app.scroll_offset(), 0);
    press(&mut app, MouseEventKind::Up(MouseButton::Left), bottom);
}

#[test]
fn test_a_menu_over_the_chat_keeps_the_rows_of_the_scrollbar_it_covers() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let (mut app, mut terminal) = overflowing_chat();
    draw(&mut app, &mut terminal);
    let bar = app.areas.scrollbar.expect("the chat overflows");
    assert_eq!(bar.open, bar.track.height, "nothing over it");

    // The slash menu opens over the bottom of the chat, its right border
    // on the scrollbar's column.
    for c in "/lo".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    draw(&mut app, &mut terminal);
    let bar = app.areas.scrollbar.expect("the chat overflows");
    assert!(bar.open > 0 && bar.open < bar.track.height, "{bar:?}");
    let covered = bar.track.y + bar.open;
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        bar.track.x,
        covered,
    ));
    assert!(!app.scrollbar_held(), "the menu's border");
    assert_eq!(app.scroll_offset(), 0);
    app.handle_mouse(mouse(
        MouseEventKind::Up(MouseButton::Left),
        bar.track.x,
        covered,
    ));

    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        bar.track.x,
        bar.track.y,
    ));
    assert!(app.scrollbar_held(), "above the menu");
    assert_eq!(app.scroll_offset(), bar.last, "jumped to the top");
}

/// An app showing a 400-line file over the chat, drawn once on a 90 × 30
/// screen, and the directory the file is in.
fn long_file_over_the_chat() -> (
    tempfile::TempDir,
    App,
    ratatui::Terminal<ratatui::backend::TestBackend>,
) {
    let dir = tempfile::tempdir().unwrap();
    let text: String = (1..=400).map(|i| format!("line {i}\n")).collect();
    std::fs::write(dir.path().join("long.txt"), text).unwrap();
    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    app.focus = PanelFocus::Sandbox;
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(app.detail_view(), Some(DetailView::File(_))));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    draw(&mut app, &mut terminal);
    (dir, app, terminal)
}

#[test]
fn test_dragging_the_scrollbar_of_a_file_over_the_chat_scrolls_the_file() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let (_dir, mut app, mut terminal) = long_file_over_the_chat();
    let bar = app.areas.scrollbar.expect("the file overflows");
    assert_eq!(bar.target, scrollbar::ScrollTarget::Detail);
    let (column, top, bottom) = (bar.track.x, bar.track.y, bar.track.bottom() - 1);

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, top));
    assert_eq!(app.detail_scroll(), 0, "pressing the thumb leaves it");
    app.handle_mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        column,
        bottom,
    ));
    assert_eq!(app.detail_scroll(), bar.last, "at the end of the file");
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, bottom));
    assert!(app.selection().is_none());
    draw(&mut app, &mut terminal);
    let bar = app.areas.scrollbar.expect("the file overflows");
    assert_eq!(bar.first, bar.last, "the cursor came along");

    // A press on the track away from the thumb jumps it there.
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, top));
    assert_eq!(app.detail_scroll(), 0, "back at the top");
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, top));
    assert!(matches!(app.detail_view(), Some(DetailView::File(_))));
}

#[test]
fn test_the_scrollbar_of_a_file_reaches_no_further_than_its_text() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let (_dir, mut app, mut terminal) = long_file_over_the_chat();
    let bar = app.areas.scrollbar.expect("the file overflows");
    let rows = app.areas.chat_rows.expect("the file's rows");
    assert_eq!(bar.track.x - rows.right(), 1, "a column of padding");
    let (padding, text) = (bar.track.x - 1, bar.track.x - 2);
    let (top, bottom) = (bar.track.y, bar.track.bottom() - 1);

    // The padding grabs it: pressed at the bottom, the thumb jumps there.
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        padding,
        bottom,
    ));
    assert!(app.scrollbar_held());
    assert_eq!(app.detail_scroll(), bar.last, "at the end of the file");
    app.handle_mouse(mouse(
        MouseEventKind::Up(MouseButton::Left),
        padding,
        bottom,
    ));

    // The text's last column is the file's.
    draw(&mut app, &mut terminal);
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), text, top));
    assert!(!app.scrollbar_held());
    assert_eq!(app.detail_scroll(), bar.last, "where it was");
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), text, top));
}
