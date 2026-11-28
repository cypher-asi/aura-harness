use super::channels::channels_app;
use super::*;

/// When a merge of the tests was queued, and when one finished.
const QUEUED_AT: &str = "2024-11-27T20:31:00Z";
const FINISHED_AT: &str = "2024-11-27T20:44:00Z";

/// `at`, RFC 3339, as QUEUE shows it: month, day and minute, local time.
fn local(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .unwrap()
        .with_timezone(&chrono::Local)
        .format("%m-%d %H:%M")
        .to_string()
}

/// [`channels_app`] with `#zode` open on its QUEUE tab.
fn queue_app() -> crate::App {
    use crate::app::{PaneTab, SidePanelTab};
    let mut app = channels_app();
    app.open_channel_by_id("c1");
    app.select_pane_tab(PaneTab::Side(SidePanelTab::ChannelQueue));
    assert_eq!(app.side_tab(), SidePanelTab::ChannelQueue);
    app
}

/// `#zode`'s queue, newest first: a running merge, a failed one and a
/// merged one.
fn zode_queue() -> crate::events::MergeQueueView {
    use crate::events::{MergeEntryView, MergeQueueView, MergeStateView};
    let entry = |id: &str, branch: &str, state| MergeEntryView {
        id: id.to_string(),
        branch: branch.to_string(),
        head: format!("{id}abc1"),
        agent: Some("RE36".to_string()),
        state,
        queued_at: QUEUED_AT.to_string(),
        ..MergeEntryView::default()
    };
    MergeQueueView {
        target: "main".to_string(),
        repo: "/src/zode".to_string(),
        entries: vec![
            entry("q3", "feat/lexer", MergeStateView::Running),
            MergeEntryView {
                finished_at: Some(FINISHED_AT.to_string()),
                attempts: 2,
                detail: Some("cargo test failed".to_string()),
                ..entry("q2", "fix/escapes", MergeStateView::Failed)
            },
            MergeEntryView {
                finished_at: Some(FINISHED_AT.to_string()),
                merged: Some("9f8e7d6".to_string()),
                ..entry("q1", "docs/readme", MergeStateView::Merged)
            },
        ],
        older: 0,
    }
}

/// Send `#zode`'s queue to `app`.
fn send_zode_queue(app: &mut crate::App) {
    app.process_command(crate::events::UiCommand::SetMergeQueue {
        channel_id: "c1".to_string(),
        queue: zode_queue(),
    });
}

#[test]
fn test_an_open_channel_lists_queue_right_after_info() {
    let mut app = channels_app();
    app.open_channel_by_id("c1");
    let rows = render_rows(&mut app, 140);
    // Five tabs: at this width MEMBERS and HISTORY take their short labels.
    let (bar, widths) = header_bar(&rows, "MBR");
    let row = &rows[bar];
    let members = char_column(row, "MBR");
    let info = char_column(row, "INFO");
    let queue = char_column(row, "QUEUE");
    assert!(members < info && info < queue, "{row}");
    // QUEUE is the cell right after INFO's: only a divider between them.
    let between: String = row.chars().skip(info + 4).take(queue - info - 4).collect();
    assert_eq!(between.trim(), "│", "{row}");
    assert!(widths.len() >= 3, "{widths:?}");
}

/// Rows of a screen tall enough for the whole queue below the channel's
/// TASKS, which shares the column.
const TALL: u16 = 70;

#[test]
fn test_queue_tab_lists_every_merge_newest_first() {
    let mut app = queue_app();
    send_zode_queue(&mut app);
    let rows = render_rows_sized(&mut app, 140, TALL);
    let header = line_with(&rows, "Merges into main");
    assert!(line_with(&rows, "/src/zode") > header);
    let running = line_with(&rows, "RUNNING");
    let failed = line_with(&rows, "FAILED");
    let merged = line_with(&rows, "MERGED");
    assert!(header < running && running < failed && failed < merged);
    for (row, branch, head) in [
        (running, "feat/lexer", "q3abc1"),
        (failed, "fix/escapes", "q2abc1"),
        (merged, "docs/readme", "q1abc1"),
    ] {
        assert!(rows[row].contains(branch), "{}", rows[row]);
        assert!(rows[row].contains(head), "{}", rows[row]);
    }
    let queued = local(QUEUED_AT);
    let queued_row = line_with(&rows, &queued);
    assert!(queued_row > running && queued_row < failed, "{queued}");
    assert!(rows[queued_row].contains("Queued"), "{}", rows[queued_row]);
    let finished = line_with(&rows, &local(FINISHED_AT));
    assert!(finished > failed && finished < merged);
    let attempts = line_with(&rows, "Attempts");
    assert!(rows[attempts].contains('2'), "{}", rows[attempts]);
    let detail = line_with(&rows, "cargo test failed");
    assert!(detail > failed && detail < merged);
    assert!(line_with(&rows, "9f8e7d6") > merged);
    // A merge that ran once does not count its attempts.
    assert_eq!(rows.iter().filter(|r| r.contains("Attempts")).count(), 1);
}

#[test]
fn test_queue_tab_colours_each_merge_by_its_state() {
    let mut app = queue_app();
    send_zode_queue(&mut app);
    let theme = test_theme();
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, TALL)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, &mut app, &theme))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let fg_of = |text: &str| {
        let row = row_of(&buffer, text);
        find_cell(&buffer, row, text).unwrap().1.fg
    };
    assert_eq!(fg_of("MERGED"), Some(theme.colors.success));
    assert_eq!(fg_of("FAILED"), Some(theme.colors.error));
    assert_eq!(fg_of("RUNNING"), Some(theme.colors.warning));
    assert_eq!(fg_of("cargo test failed"), Some(theme.colors.error));
}

#[test]
fn test_queue_tab_without_merges_says_where_they_come_from() {
    use crate::events::{MergeQueueView, UiCommand};
    let mut app = queue_app();
    let screen = render_rows(&mut app, 140).join("\n");
    assert!(screen.contains("No merges yet."), "{screen}");
    assert!(!screen.contains("Merges into"), "{screen}");

    app.process_command(UiCommand::SetMergeQueue {
        channel_id: "c1".to_string(),
        queue: MergeQueueView {
            target: "main".to_string(),
            repo: "/src/zode".to_string(),
            entries: Vec::new(),
            older: 0,
        },
    });
    let rows = render_rows(&mut app, 140);
    let header = line_with(&rows, "Merges into main");
    assert!(line_with(&rows, "No merges yet.") > header);
}

#[test]
fn test_an_empty_queue_names_its_own_target() {
    use crate::events::{MergeQueueView, UiCommand};
    let mut app = queue_app();
    app.process_command(UiCommand::SetMergeQueue {
        channel_id: "c1".to_string(),
        queue: MergeQueueView {
            target: "trunk".to_string(),
            repo: "/src/zode".to_string(),
            ..MergeQueueView::default()
        },
    });
    let screen = render_rows(&mut app, 200).join("\n");
    assert!(screen.contains("Merges into trunk"), "{screen}");
    let queue = app.open_channel_queue().cloned();
    let lines = super::super::channel_queue::queue_lines(queue.as_ref(), 400, &test_theme());
    let text: String = lines.iter().map(ToString::to_string).collect();
    assert!(text.contains("merges into `trunk` one at a time"), "{text}");
    assert!(!text.contains("main"), "{text}");
}

#[test]
fn test_queue_tab_is_picked_by_click_and_shift_arrows_cycle_the_channel_tabs() {
    use crate::app::{PanelFocus, SidePanelTab};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
    let mut app = channels_app();
    app.open_channel_by_id("c1");
    assert_eq!(app.side_tab(), SidePanelTab::ChannelMembers);
    let rows = render_rows(&mut app, 140);
    let bar = line_with(&rows, "QUEUE");
    let column = char_column(&rows[bar], "QUEUE");
    let at = |n: usize| u16::try_from(n).unwrap();
    let click = MouseEventKind::Down(MouseButton::Left);
    mouse_at(&mut app, click, (at(column + 1), at(bar)));
    assert_eq!(app.side_tab(), SidePanelTab::ChannelQueue);
    let screen = render_rows(&mut app, 140).join("\n");
    assert!(screen.contains("No merges yet."), "{screen}");

    for _ in 0..8 {
        if app.focus() == PanelFocus::Sandbox {
            break;
        }
        let _ = app.handle_key(KeyEvent::from(KeyCode::Tab));
    }
    assert_eq!(app.focus(), PanelFocus::Sandbox);
    let mut shown = Vec::new();
    for _ in 0..5 {
        let _ = app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        shown.push(app.side_tab());
    }
    assert_eq!(
        shown,
        [
            SidePanelTab::Tasks,
            SidePanelTab::ChannelHistory,
            SidePanelTab::ChannelMembers,
            SidePanelTab::ChannelInfo,
            SidePanelTab::ChannelQueue
        ]
    );
}

#[test]
fn test_a_time_that_does_not_parse_is_cut_to_its_minutes() {
    use super::super::channel_queue::compact_time;
    assert_eq!(
        compact_time("2024-11-27T20:31:59+bogus"),
        "2024-11-27 20:31"
    );
    assert_eq!(compact_time(QUEUED_AT), local(QUEUED_AT));
}

/// `#zode`'s queue with `older` merges not sent yet.
fn paged_queue(older: u64) -> crate::events::MergeQueueView {
    crate::events::MergeQueueView {
        older,
        ..zode_queue()
    }
}

#[test]
fn test_older_merges_are_counted_and_asked_for_once_at_the_end() {
    use crate::events::{UiCommand, UiEvent};
    let mut app = queue_app();
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    app.link.event_tx = Some(tx);
    app.process_command(UiCommand::SetMergeQueue {
        channel_id: "c1".to_string(),
        queue: paged_queue(150),
    });
    // Tall enough that the whole tab, its end too, is on screen.
    let screen = render_rows_sized(&mut app, 140, 60).join("\n");
    assert!(screen.contains("150 older merges"), "{screen}");
    render_rows_sized(&mut app, 140, 60);
    let asked: Vec<UiEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert_eq!(asked.len(), 1, "once per page shown: {asked:?}");
    assert!(
        matches!(&asked[0], UiEvent::LoadOlderMerges { channel_id } if channel_id == "c1"),
        "{asked:?}"
    );

    // The next page came: one more ask once its end shows.
    let mut more = paged_queue(50);
    more.entries.extend(zode_queue().entries);
    app.process_command(UiCommand::SetMergeQueue {
        channel_id: "c1".to_string(),
        queue: more,
    });
    render_rows_sized(&mut app, 140, 80);
    assert_eq!(std::iter::from_fn(|| rx.try_recv().ok()).count(), 1);
}

#[test]
fn test_a_whole_history_asks_for_nothing() {
    use crate::events::UiCommand;
    let mut app = queue_app();
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    app.link.event_tx = Some(tx);
    app.process_command(UiCommand::SetMergeQueue {
        channel_id: "c1".to_string(),
        queue: paged_queue(0),
    });
    let screen = render_rows_sized(&mut app, 140, 60).join("\n");
    assert!(!screen.contains("older merge"), "{screen}");
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_older_merges_wait_until_the_tab_is_scrolled_to_its_end() {
    use crate::events::UiCommand;
    let mut app = queue_app();
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    app.link.event_tx = Some(tx);
    let mut long = paged_queue(10);
    for _ in 0..6 {
        long.entries.extend(zode_queue().entries);
    }
    app.process_command(UiCommand::SetMergeQueue {
        channel_id: "c1".to_string(),
        queue: long,
    });
    render_rows_sized(&mut app, 140, 30);
    assert!(rx.try_recv().is_err(), "the end is not on screen yet");
}

#[test]
fn test_queue_tab_lists_an_entrys_overlaps_under_it() {
    use crate::events::{MergeEntryView, UiCommand};
    let mut app = queue_app();
    let mut queue = zode_queue();
    queue.entries[0] = MergeEntryView {
        overlaps: vec![
            "q2 fix/escapes (RE36): src/lexer.rs".to_string(),
            "worktree feat/tokens (RE37): src/lexer.rs".to_string(),
        ],
        ..queue.entries[0].clone()
    };
    app.process_command(UiCommand::SetMergeQueue {
        channel_id: "c1".to_string(),
        queue,
    });
    let rows = render_rows_sized(&mut app, 140, TALL);
    let running = line_with(&rows, "RUNNING");
    let failed = line_with(&rows, "FAILED");
    // The side panel is narrow: each overlap wraps, so look for its start.
    let first = line_with(&rows, "q2 fix/escapes");
    let second = line_with(&rows, "worktree");
    assert!(running < first && first < second && second < failed);
    assert!(rows[first].contains("Overlaps"), "{}", rows[first]);
    assert!(!rows[second].contains("Overlaps"), "{}", rows[second]);
    assert_eq!(rows.iter().filter(|r| r.contains("Overlaps")).count(), 1);
}
