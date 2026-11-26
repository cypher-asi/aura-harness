
use super::*;
use crate::events::{AgentSummary, ProcessKind, ProcessStatus, ProcessSummary, UiCommand};
use crossterm::event::KeyModifiers;

const LEAD: &str = "aa";

fn mail(id: u64, thread: &str, from: &str, to: &str, sent_ms: u64) -> MailView {
    MailView {
        id,
        thread_id: thread.into(),
        subject: format!("About {thread}"),
        from_id: from.into(),
        from_name: from.to_uppercase(),
        to_id: to.into(),
        to_name: to.to_uppercase(),
        reply: false,
        text: format!("message {id}\nsecond line"),
        sent_ms,
    }
}

fn task(id: &str, owner: &str) -> ProcessSummary {
    ProcessSummary {
        id: format!("task:{id}"),
        kind: ProcessKind::Task,
        label: id.into(),
        status: ProcessStatus::Running,
        owner_agent_id: owner.into(),
        started_ms: 10,
        ..ProcessSummary::default()
    }
}

fn app_with_lead() -> App {
    let mut app = App::new();
    app.process_command(UiCommand::SetAgents(vec![AgentSummary {
        id: LEAD.into(),
        name: "Lead".into(),
        ..AgentSummary::default()
    }]));
    app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
    app.process_command(UiCommand::SetProcesses(vec![task("bb", LEAD)]));
    app
}

fn send(app: &mut App, mail: MailView) {
    app.process_command(UiCommand::AddMail(mail));
}

fn threads(app: &App) -> Vec<String> {
    app.inbox_rows()
        .iter()
        .filter_map(|row| match row {
            InboxRow::Thread { thread_id, .. } => Some(thread_id.clone()),
            InboxRow::Message { .. } => None,
        })
        .collect()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

/// The thread shown over the chat and its highlighted message.
fn open_thread(app: &App) -> Option<(String, Option<u64>)> {
    let view = app.thread_view()?;
    Some((view.thread_id.clone(), view.selected))
}

fn unfolded(app: &App) -> Vec<u64> {
    let mut ids: Vec<u64> = app
        .thread_view()
        .map(|view| view.unfolded.iter().copied().collect())
        .unwrap_or_default();
    ids.sort_unstable();
    ids
}

/// Lead's run with thread t1 of messages 1, 2 and 3 on screen,
/// opened at message 1.
fn app_with_open_thread() -> App {
    let mut app = app_with_lead();
    send(&mut app, mail(1, "t1", LEAD, "bb", 100));
    send(&mut app, mail(2, "t1", "bb", LEAD, 200));
    send(&mut app, mail(3, "t1", LEAD, "bb", 300));
    app.open_thread("t1".into(), Some(1));
    app
}

#[test]
fn messages_group_by_thread_and_threads_order_by_last_activity() {
    let mut app = app_with_lead();
    send(&mut app, mail(1, "t1", LEAD, "bb", 100));
    send(&mut app, mail(2, "t2", LEAD, "bb", 200));
    send(&mut app, mail(3, "t1", "bb", LEAD, 300));
    assert_eq!(threads(&app), ["t1", "t2"]);
    assert!(matches!(
        &app.inbox_rows()[0],
        InboxRow::Thread { count: 2, participants, last_ms: 300, expanded: false, .. }
            if participants == "AA ↔ BB"
    ));
    assert!(matches!(
        &app.inbox_rows()[1],
        InboxRow::Thread { count: 1, participants, .. } if participants == "AA → BB"
    ));
}

#[test]
fn only_threads_of_the_run_on_screen_are_listed() {
    let mut app = app_with_lead();
    send(&mut app, mail(1, "ours", "bb", LEAD, 100));
    send(&mut app, mail(2, "theirs", "ff", "gg", 200));
    assert_eq!(threads(&app), ["ours"]);

    // A member joining brings its threads in.
    app.process_command(UiCommand::SetProcesses(vec![
        task("bb", LEAD),
        task("ff", LEAD),
    ]));
    assert_eq!(threads(&app), ["theirs", "ours"]);

    // Another agent on screen lists its own run's threads.
    app.process_command(UiCommand::SetActiveAgent("zz".into()));
    assert!(threads(&app).is_empty());
}

#[test]
fn every_thread_is_listed_with_no_agent_on_screen() {
    let mut app = App::new();
    send(&mut app, mail(1, "t1", "ff", "gg", 100));
    assert_eq!(threads(&app), ["t1"]);
}

#[test]
fn right_unfolds_a_thread_and_left_folds_it_from_a_message() {
    let mut app = app_with_lead();
    send(&mut app, mail(1, "t1", LEAD, "bb", 100));
    send(&mut app, mail(2, "t1", "bb", LEAD, 200));
    app.show_inbox();
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.inbox_rows().len(), 3);
    assert!(matches!(
        &app.inbox_rows()[1],
        InboxRow::Message { message_id: 1, preview, .. } if preview == "message 1 second line"
    ));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.selected_inbox_row(), 2);
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.inbox_rows().len(), 1);
    assert_eq!(app.selected_inbox_row(), 0);
    assert!(matches!(
        &app.inbox_rows()[0],
        InboxRow::Thread {
            expanded: false,
            ..
        }
    ));
}

#[test]
fn enter_on_a_message_opens_its_thread_there_and_marks_it_read() {
    let mut app = app_with_lead();
    send(&mut app, mail(1, "t1", LEAD, "bb", 100));
    send(&mut app, mail(2, "t1", "bb", LEAD, 200));
    send(&mut app, mail(3, "t2", "bb", LEAD, 50));
    assert_eq!(app.inbox_header_counts(), (2, 2));
    app.show_inbox();
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(open_thread(&app), Some(("t1".into(), Some(1))));
    assert_eq!(app.inbox_header_counts(), (1, 2));

    // A thread row opens at its newest message.
    app.handle_key(key(KeyCode::Esc));
    assert!(app.handle_records_shortcut(key(KeyCode::End)));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(open_thread(&app), Some(("t2".into(), Some(3))));
    assert_eq!(app.inbox_header_counts(), (0, 2));
}

#[test]
fn new_mail_makes_a_read_thread_unread() {
    let mut app = app_with_lead();
    send(&mut app, mail(1, "t1", LEAD, "bb", 100));
    app.open_thread("t1".into(), None);
    assert_eq!(app.inbox_header_counts(), (0, 1));
    app.handle_key(key(KeyCode::Esc));
    assert!(app.detail_view().is_none());
    send(&mut app, mail(2, "t1", "bb", LEAD, 200));
    assert_eq!(app.inbox_header_counts(), (1, 1));
}

#[test]
fn clicking_a_row_selects_and_opens_it() {
    let mut app = app_with_lead();
    send(&mut app, mail(1, "t1", LEAD, "bb", 100));
    send(&mut app, mail(2, "t2", LEAD, "bb", 200));
    app.show_inbox();
    app.set_inbox_rows_area(Rect::new(80, 3, 40, 10), 0);
    app.click_records_panel(85, 4);
    assert_eq!(app.selected_inbox_row(), 1);
    assert_eq!(open_thread(&app), Some(("t1".into(), Some(1))));
}

#[test]
fn the_highlight_stays_on_its_thread_as_mail_arrives() {
    let mut app = app_with_lead();
    send(&mut app, mail(1, "t1", LEAD, "bb", 100));
    send(&mut app, mail(2, "t2", LEAD, "bb", 200));
    app.show_inbox();
    app.handle_key(key(KeyCode::Down));
    send(&mut app, mail(3, "t3", LEAD, "bb", 300));
    let selected = &app.inbox_rows()[app.selected_inbox_row()];
    assert_eq!(selected.thread_id(), "t1");
}

#[test]
fn shift_right_cycles_through_all_four_tabs() {
    let mut app = app_with_lead();
    let right = KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT);
    let left = KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT);
    for tab in [
        RecordsTab::Members,
        RecordsTab::Policies,
        RecordsTab::Inbox,
        RecordsTab::Kernel,
    ] {
        assert!(app.handle_records_shortcut(right));
        assert_eq!(app.records_tab(), tab);
    }
    assert!(app.handle_records_shortcut(left));
    assert_eq!(app.records_tab(), RecordsTab::Inbox);
}

#[test]
fn the_inbox_command_shows_the_tab_focused() {
    for command in ["/inbox", "/mail"] {
        let mut app = app_with_lead();
        app.handle_command("/kernel");
        assert!(!app.record_panel_visible());
        app.handle_command(command);
        assert!(app.record_panel_visible());
        assert_eq!(app.records_tab(), RecordsTab::Inbox);
        assert_eq!(app.focus(), PanelFocus::Records);
    }
}

#[test]
fn mail_is_deduplicated_by_id_and_capped() {
    let mut app = App::new();
    send(&mut app, mail(1, "t1", "ff", "gg", 100));
    send(&mut app, mail(1, "t1", "ff", "gg", 100));
    assert!(matches!(
        &app.inbox_rows()[0],
        InboxRow::Thread { count: 1, .. }
    ));

    for id in 2..=u64::try_from(MAX_MAIL).unwrap_or(u64::MAX) + 1 {
        send(&mut app, mail(id, "t2", "ff", "gg", 200 + id));
    }
    assert_eq!(app.inbox.messages.len(), MAX_MAIL);
    assert_eq!(threads(&app), ["t2"], "the oldest message went first");
}

#[test]
fn capped_mail_forgets_the_state_of_threads_it_dropped() {
    let mut app = App::new();
    send(&mut app, mail(1, "old", "ff", "gg", 100));
    send(&mut app, mail(2, "kept", "ff", "gg", 100));
    app.open_thread("old".into(), None);
    app.open_thread("kept".into(), None);
    app.inbox.expanded.insert("old".into());
    app.inbox.expanded.insert("kept".into());

    for id in 3..=u64::try_from(MAX_MAIL).unwrap_or(u64::MAX) + 1 {
        send(&mut app, mail(id, "new", "ff", "gg", 200 + id));
    }
    assert_eq!(threads(&app), ["new", "kept"]);
    assert!(!app.inbox.expanded.contains("old"));
    assert!(!app.inbox.read.contains_key("old"));
    assert!(app.inbox.expanded.contains("kept"));
    assert!(app.inbox.read.contains_key("kept"));
}

#[test]
fn opening_a_thread_unfolds_and_selects_the_focused_and_newest_message() {
    let app = app_with_open_thread();
    let Some(DetailView::Thread(view)) = app.detail_view() else {
        panic!("no thread view");
    };
    assert_eq!(view.subject, "About t1");
    assert_eq!(view.selected, Some(1));
    assert_eq!(unfolded(&app), [1, 3]);
    assert_eq!(
        app.thread_messages("t1")
            .iter()
            .map(|mail| mail.id)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

#[test]
fn opening_a_thread_without_focus_selects_its_newest_message() {
    let mut app = app_with_open_thread();
    app.open_thread("t1".into(), Some(99));
    assert_eq!(open_thread(&app), Some(("t1".into(), Some(3))));
    assert_eq!(unfolded(&app), [3]);
}

#[test]
fn thread_keys_move_the_selection_and_fold_messages() {
    let mut app = app_with_open_thread();
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(unfolded(&app), [3], "Enter folds the selected message");
    assert!(app.detail_view().is_some(), "Enter does not close the view");
    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(unfolded(&app), [1, 3], "Space unfolds it again");

    app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(open_thread(&app), Some(("t1".into(), Some(2))));
    app.handle_key(key(KeyCode::Char('n')));
    app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(
        open_thread(&app),
        Some(("t1".into(), Some(3))),
        "n stops at the last"
    );
    app.handle_key(key(KeyCode::Char('p')));
    assert_eq!(open_thread(&app), Some(("t1".into(), Some(2))));

    app.handle_key(key(KeyCode::Char('e')));
    assert_eq!(unfolded(&app), [1, 2, 3]);
    app.handle_key(key(KeyCode::Char('c')));
    assert!(unfolded(&app).is_empty());
    assert_eq!(app.input(), "", "thread keys stay out of the input");
}

#[test]
fn thread_selection_scrolls_to_the_selected_header() {
    let mut app = app_with_open_thread();
    app.set_detail_bounds(100, 1);
    // Message 1 unfolded: header + 2 body lines, then a blank line.
    app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(app.detail_scroll(), 4);
    app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(app.detail_scroll(), 6);
}

#[test]
fn new_mail_in_the_open_thread_shows_unfolded_and_read() {
    let mut app = app_with_open_thread();
    send(&mut app, mail(4, "t1", "bb", LEAD, 400));
    send(&mut app, mail(5, "t2", "bb", LEAD, 500));
    assert_eq!(unfolded(&app), [1, 3, 4]);
    assert_eq!(app.inbox_header_counts(), (1, 2), "only t2 is unread");
}

#[test]
fn clicking_a_thread_header_toggles_its_message() {
    let mut app = app_with_open_thread();
    app.set_chat_rows(Some((Rect::new(2, 1, 40, 20), 0)));
    app.set_thread_header_rows(vec![(0, 1), (4, 2), (6, 3)]);
    assert!(app.click_thread(5, 5));
    assert_eq!(open_thread(&app), Some(("t1".into(), Some(2))));
    assert_eq!(unfolded(&app), [1, 2, 3]);
    assert!(app.click_thread(5, 1));
    assert_eq!(unfolded(&app), [2, 3]);
    assert!(!app.click_thread(5, 2), "a body line is not a header");
}

fn name_thread(app: &mut App, thread: &str, subject: &str) {
    app.process_command(UiCommand::SetThreadSubject {
        thread_id: thread.into(),
        subject: subject.into(),
    });
}

/// Subject of the row of `thread`, if it is listed.
fn row_subject(app: &App, thread: &str) -> Option<String> {
    app.inbox_rows().iter().find_map(|row| match row {
        InboxRow::Thread {
            thread_id, subject, ..
        } if thread_id == thread => Some(subject.clone()),
        _ => None,
    })
}

#[test]
fn a_subject_after_the_mail_replaces_the_row_subject() {
    let mut app = app_with_lead();
    send(&mut app, mail(1, "t1", LEAD, "bb", 100));
    assert_eq!(row_subject(&app, "t1").as_deref(), Some("About t1"));
    name_thread(&mut app, "t1", "  Review parser escapes \n");
    assert_eq!(
        row_subject(&app, "t1").as_deref(),
        Some("Review parser escapes")
    );
}

#[test]
fn a_subject_before_the_first_mail_is_used_when_it_arrives() {
    let mut app = app_with_lead();
    name_thread(&mut app, "t1", "Benchmark results for Gate");
    assert!(threads(&app).is_empty());
    send(&mut app, mail(1, "t1", LEAD, "bb", 100));
    assert_eq!(
        row_subject(&app, "t1").as_deref(),
        Some("Benchmark results for Gate")
    );
}

#[test]
fn later_mail_keeps_the_written_subject() {
    let mut app = app_with_lead();
    send(&mut app, mail(1, "t1", LEAD, "bb", 100));
    name_thread(&mut app, "t1", "Inbox subject titles");
    send(&mut app, mail(2, "t1", "bb", LEAD, 200));
    assert_eq!(
        row_subject(&app, "t1").as_deref(),
        Some("Inbox subject titles")
    );
    app.open_thread("t1".into(), None);
    let Some(DetailView::Thread(view)) = app.detail_view() else {
        panic!("no thread view");
    };
    assert_eq!(view.subject, "Inbox subject titles");
}

#[test]
fn a_subject_renames_the_open_thread_view() {
    let mut app = app_with_open_thread();
    name_thread(&mut app, "t2", "Another thread");
    name_thread(&mut app, "t1", "Review parser escapes");
    let Some(DetailView::Thread(view)) = app.detail_view() else {
        panic!("no thread view");
    };
    assert_eq!(view.subject, "Review parser escapes");
}

#[test]
fn a_blank_subject_is_ignored() {
    let mut app = app_with_open_thread();
    name_thread(&mut app, "t1", " \n ");
    assert_eq!(row_subject(&app, "t1").as_deref(), Some("About t1"));
    let Some(DetailView::Thread(view)) = app.detail_view() else {
        panic!("no thread view");
    };
    assert_eq!(view.subject, "About t1");
    assert!(!app.inbox.subjects.contains_key("t1"));
}

#[test]
fn capped_mail_forgets_subjects_of_dropped_threads_only() {
    let mut app = App::new();
    send(&mut app, mail(1, "old", "ff", "gg", 100));
    send(&mut app, mail(2, "kept", "ff", "gg", 100));
    name_thread(&mut app, "old", "Old thread");
    name_thread(&mut app, "kept", "Kept thread");
    name_thread(&mut app, "early", "Early thread");

    for id in 3..=u64::try_from(MAX_MAIL).unwrap_or(u64::MAX) + 1 {
        send(&mut app, mail(id, "new", "ff", "gg", 200 + id));
    }
    assert!(!app.inbox.subjects.contains_key("old"));
    assert!(app.inbox.subjects.contains_key("kept"));
    assert!(
        app.inbox.subjects.contains_key("early"),
        "a subject waiting for its first message is kept"
    );
}
