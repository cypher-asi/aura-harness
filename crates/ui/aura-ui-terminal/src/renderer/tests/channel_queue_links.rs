//! QUEUE's branch links and times: a branch opens on GitHub and
//! underlines under the pointer, times read relative, show in full under
//! the pointer, and redraw the tab only once due.

use super::channel_queue::{queue_app, queue_buffer, zode_queue, TALL};
use super::*;

/// `#zode`'s queue, in a repo on GitHub, drawn on a tall screen; returns
/// the screen and the row and column the branch `branch` starts at.
fn linked_queue(app: &mut crate::App, web: Option<&str>, branch: &str) -> (u16, u16) {
    app.process_command(crate::events::UiCommand::SetMergeQueue {
        channel_id: "c1".to_string(),
        queue: crate::events::MergeQueueView {
            web: web.map(str::to_string),
            ..zode_queue()
        },
    });
    let rows = render_rows_sized(app, 140, TALL);
    let row = line_with(&rows, branch);
    let column = char_column(&rows[row], branch);
    (u16::try_from(column).unwrap(), u16::try_from(row).unwrap())
}

#[test]
fn test_a_click_on_a_branch_opens_it_on_github() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let mut app = queue_app();
    let at = linked_queue(&mut app, Some("https://github.com/o/r"), "fix/escapes");
    mouse_at(&mut app, MouseEventKind::Down(MouseButton::Left), at);
    assert_eq!(
        app.opened_links(),
        ["https://github.com/o/r/tree/fix/escapes"]
    );
    // The header above it is no link.
    mouse_at(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        (at.0, at.1 - 1),
    );
    assert_eq!(app.opened_links().len(), 1);
}

#[test]
fn test_a_branch_of_a_repo_off_github_opens_nothing() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let mut app = queue_app();
    let at = linked_queue(&mut app, None, "fix/escapes");
    mouse_at(&mut app, MouseEventKind::Down(MouseButton::Left), at);
    assert!(app.opened_links().is_empty());
}

/// `#zode`'s queue with its newest merge queued `secs` ago, in a repo on
/// GitHub.
fn recent_queue(secs: i64) -> crate::events::MergeQueueView {
    let mut queue = zode_queue();
    let at = chrono::Local::now() - chrono::Duration::seconds(secs);
    queue.entries[0].queued_at = at.to_rfc3339();
    queue.web = Some("https://github.com/o/r".to_string());
    queue
}

#[test]
fn test_queue_times_read_relative_and_say_when_they_next_change() {
    use super::super::channel_queue::{queue_doc, QueueLook};
    let queue = recent_queue(90);
    let now = chrono::Local::now();
    let doc = queue_doc(Some(&queue), 60, &test_theme(), QueueLook::at(now));
    let text: Vec<String> = doc.lines.iter().map(ToString::to_string).collect();
    let queued = &text[doc.times[0]];
    assert!(
        queued.contains("Queued") && queued.contains("1 minute ago"),
        "{queued}"
    );
    let due = doc.due.expect("a time that still changes").as_secs();
    assert!(
        (29..=31).contains(&due),
        "the next minute, not the next frame: {due}"
    );
    // Every merge of 2024 has a time; the old ones read as dates.
    assert!(
        text[doc.times[1]].contains("2024"),
        "{}",
        text[doc.times[1]]
    );
}

#[test]
fn test_the_pointer_on_a_time_shows_it_in_full_and_on_a_branch_underlines_it() {
    use crossterm::event::MouseEventKind;
    use ratatui::style::Modifier;
    let mut app = queue_app();
    app.process_command(crate::events::UiCommand::SetMergeQueue {
        channel_id: "c1".to_string(),
        queue: recent_queue(90),
    });
    let theme = test_theme();
    let buffer = queue_buffer(&mut app, &theme);
    let time_row = row_of(&buffer, "1 minute ago");
    let (time_x, _) = find_cell(&buffer, time_row, "1 minute ago").unwrap();
    let branch_row = row_of(&buffer, "feat/lexer");
    let (branch_x, branch) = find_cell(&buffer, branch_row, "feat/lexer").unwrap();
    assert!(
        !branch.add_modifier.contains(Modifier::UNDERLINED),
        "plain at rest"
    );

    mouse_at(&mut app, MouseEventKind::Moved, (time_x, time_row));
    let rows = render_rows_sized(&mut app, 140, TALL);
    let row = &rows[usize::from(time_row)];
    assert!(!row.contains("minute ago"), "{row}");
    let queued = app.open_channel_queue().unwrap().entries[0]
        .queued_at
        .clone();
    let exact = crate::components::exact_time(&crate::components::parse_time(&queued).unwrap());
    assert!(row.contains(&exact[..10]), "the full time, {exact}: {row}");
    let next = &rows[usize::from(time_row) + 1];
    assert!(next.contains(&exact[11..19]), "to the second: {next}");

    mouse_at(&mut app, MouseEventKind::Moved, (branch_x, branch_row));
    let buffer = queue_buffer(&mut app, &theme);
    assert!(
        row_of(&buffer, "1 minute ago") == time_row,
        "the time reads relative again"
    );
    let (_, branch) = find_cell(&buffer, branch_row, "feat/lexer").unwrap();
    assert!(
        branch.add_modifier.contains(Modifier::UNDERLINED),
        "underlined on hover"
    );
}

#[test]
fn test_the_tab_is_drawn_again_only_once_a_time_is_due() {
    use std::time::{Duration, Instant};
    let mut app = queue_app();
    render_rows(&mut app, 140);
    app.set_channel_queue_marks(
        Vec::new(),
        Vec::new(),
        Some(Instant::now() + Duration::from_secs(60)),
    );
    app.take_redraw();
    app.tick();
    assert!(!app.take_redraw(), "not before it is due");
    app.set_channel_queue_marks(
        Vec::new(),
        Vec::new(),
        Some(Instant::now() - Duration::from_secs(1)),
    );
    app.tick();
    assert!(app.take_redraw(), "once it is due");
    app.tick();
    assert!(
        !app.take_redraw(),
        "once only, until the next draw records the next"
    );
}
