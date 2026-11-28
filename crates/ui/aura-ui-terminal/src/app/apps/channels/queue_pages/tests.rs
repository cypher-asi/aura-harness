use super::*;
use crate::events::{ChannelSummary, MergeEntryView, MergeStateView, UiCommand};
use tokio::sync::mpsc;

fn merge(n: u64) -> MergeEntryView {
    MergeEntryView {
        id: format!("q{n}"),
        branch: format!("aura/b{n}"),
        state: MergeStateView::Merged,
        ..MergeEntryView::default()
    }
}

fn view(entries: Vec<MergeEntryView>, older: Option<u64>, page_before: Option<u64>) -> UiCommand {
    UiCommand::SetMergeQueue {
        channel_id: "c1".to_string(),
        queue: MergeQueueView {
            target: "main".to_string(),
            entries,
            older,
            page_before,
            ..MergeQueueView::default()
        },
    }
}

/// `#zode` open, and what the app asks of the session.
fn open_app() -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(16);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetChannels(vec![ChannelSummary {
        id: "c1".to_string(),
        slug: "zode".to_string(),
        coordinator_id: "co".to_string(),
        ..ChannelSummary::default()
    }]));
    app.open_channel_by_id("c1");
    let mut rx = rx;
    while rx.try_recv().is_ok() {}
    (app, rx)
}

fn ids(app: &App) -> Vec<String> {
    app.channel_queue("c1")
        .map(|q| q.entries.iter().map(|e| e.id.clone()).collect())
        .unwrap_or_default()
}

fn asked(rx: &mut mpsc::Receiver<UiEvent>) -> Vec<u64> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let UiEvent::LoadMergeHistory { channel_id, before } = event {
            assert_eq!(channel_id, "c1");
            out.push(before);
        }
    }
    out
}

#[test]
fn older_merges_are_asked_for_once_per_cursor_and_added_after_those_shown() {
    let (mut app, mut rx) = open_app();
    app.process_command(view(vec![merge(5), merge(4)], Some(300), None));
    app.request_older_merges();
    app.request_older_merges();
    assert_eq!(asked(&mut rx), [300], "asked once");

    app.process_command(view(vec![merge(3), merge(2)], Some(100), Some(300)));
    assert_eq!(ids(&app), ["q5", "q4", "q3", "q2"]);
    app.request_older_merges();
    assert_eq!(asked(&mut rx), [100]);

    app.process_command(view(vec![merge(1)], None, Some(100)));
    assert_eq!(ids(&app), ["q5", "q4", "q3", "q2", "q1"]);
    app.request_older_merges();
    assert!(asked(&mut rx).is_empty(), "the history is all here");
}

#[test]
fn a_stale_page_changes_nothing() {
    let (mut app, _rx) = open_app();
    app.process_command(view(vec![merge(5)], Some(300), None));
    app.process_command(view(vec![merge(9)], None, Some(999)));
    assert_eq!(ids(&app), ["q5"]);
    assert_eq!(app.channel_queue("c1").unwrap().older, Some(300));
}

#[test]
fn a_change_keeps_the_older_pages_loaded() {
    let (mut app, _rx) = open_app();
    app.process_command(view(vec![merge(5), merge(4)], Some(300), None));
    app.process_command(view(vec![merge(3), merge(2)], Some(100), Some(300)));
    // A merge finished: the newest page moves on by one.
    app.process_command(view(vec![merge(6), merge(5)], Some(400), None));
    assert_eq!(ids(&app), ["q6", "q5", "q4", "q3", "q2"]);
    assert_eq!(app.channel_queue("c1").unwrap().older, Some(100));
}

#[test]
fn without_older_pages_a_change_replaces_the_view() {
    let (mut app, _rx) = open_app();
    app.process_command(view(vec![merge(5), merge(4)], Some(300), None));
    app.process_command(view(vec![merge(6), merge(5)], Some(400), None));
    assert_eq!(ids(&app), ["q6", "q5"]);
    assert_eq!(app.channel_queue("c1").unwrap().older, Some(400));
}
