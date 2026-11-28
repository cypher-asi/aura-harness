//! An empty channel's chat: its slug as a banner over its coordinator's
//! welcome, until the channel has something in it.

use super::channels::channels_app;
use super::*;
use crate::events::{ChannelPostAuthor, ChannelPostView, UiCommand};

/// The channels app with `#zode` open on its coordinator's chat.
fn empty_channel() -> crate::App {
    let mut app = channels_app();
    app.process_command(UiCommand::SetActiveAgent("co".to_string()));
    app.open_channel_by_id("c1");
    app
}

/// Whether `rows` show the banner: the half-block or full-block pixels
/// the font draws.
fn shows_banner(rows: &[String]) -> bool {
    rows.iter()
        .any(|row| row.contains("██") || row.contains('▀'))
}

#[test]
fn an_empty_channel_shows_its_slug_as_a_banner_over_the_welcome() {
    let mut app = empty_channel();
    let rows = render_rows_sized(&mut app, 140, 40);
    let screen = rows.join("\n");
    assert!(shows_banner(&rows), "{screen}");
    let row_with = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} in:\n{screen}"))
    };
    let welcome = row_with("Welcome to #zode: Ship it.");
    let start = row_with("I'm Coordinator, the coordinator.");
    let hint = row_with("Mention a member with @CODE");
    let art = rows.iter().position(|row| row.contains("██")).unwrap();
    assert!(art < welcome && welcome < start && start < hint, "{screen}");
}

#[test]
fn a_channel_with_a_post_shows_its_chat_instead() {
    let mut app = empty_channel();
    app.process_command(UiCommand::SetChannelPosts {
        channel_id: "c1".to_string(),
        posts: vec![ChannelPostView {
            seq: 1,
            author: ChannelPostAuthor::User,
            text: "first message here".to_string(),
            at_ms: 1000,
            ..ChannelPostView::default()
        }],
    });
    let rows = render_rows_sized(&mut app, 140, 40);
    let screen = rows.join("\n");
    assert!(screen.contains("first message here"), "{screen}");
    assert!(!screen.contains("Welcome to #zode"), "{screen}");
    assert!(!shows_banner(&rows), "{screen}");
}

#[test]
fn a_members_own_chat_shows_no_welcome() {
    let mut app = channels_app();
    app.process_command(UiCommand::SetActiveAgent("wk".to_string()));
    app.open_channel_by_id("c1");
    let screen = render_rows_sized(&mut app, 140, 40).join("\n");
    assert!(!screen.contains("Welcome to #zode"), "{screen}");
}

#[test]
fn a_narrow_or_short_screen_keeps_the_layout() {
    for (width, height) in [(140, 12), (90, 30), (60, 20), (40, 16), (24, 10)] {
        let mut app = empty_channel();
        let rows = render_rows_sized(&mut app, width, height);
        assert_eq!(rows.len(), usize::from(height));
        for row in &rows {
            assert_eq!(row.chars().count(), usize::from(width), "{row:?}");
        }
        // The panels' frames still close on the last rows.
        let screen = rows.join("\n");
        assert!(screen.contains('└'), "{width}x{height}:\n{screen}");
    }
}
