//! HISTORY lists the messages the user typed to the coordinator, which
//! stay in its conversation and are no posts, among the user's posts.

use super::channels::channels_app;
use super::*;
use crate::app::{HistoryKind, PaneTab, PanelFocus, SidePanelTab};
use crate::components::MessageRole;
use crate::events::{prompt_key, ChannelPostAuthor, ChannelPostView, MessageData, UiCommand};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const LONG: &str = "Please make every link in the footer open in a new tab, and while you \
                    are there check the privacy link points at the new policy page as well";

/// 2024-01-02, long before today.
const DAY: u64 = 1_704_200_000_000;

fn at(role: MessageRole, text: &str, ms: u64) -> MessageData {
    MessageData {
        timestamp_ms: Some(DAY + ms),
        ..MessageData::new(role, text)
    }
}

/// `#zode` open on its coordinator's chat: two messages typed to the
/// coordinator far back, mail from an agent, a long reply after them, and
/// one post to a member, on the HISTORY tab.
fn typed_app() -> crate::App {
    let mut app = channels_app();
    app.process_command(UiCommand::SetActiveAgent("co".to_string()));
    app.open_channel_by_id("c1");
    let mut history = vec![
        at(MessageRole::User, LONG, 1_000),
        at(MessageRole::Assistant, "On it.", 2_000),
        at(
            MessageRole::User,
            "WK1: Footer done\n(inbox reply 3 from WK1, Footer)\nAll links fixed.",
            3_000,
        ),
        at(MessageRole::User, "now ship it", 4_000),
    ];
    history.extend((0..80).map(|n| at(MessageRole::Assistant, &format!("note {n}"), 10_000 + n)));
    app.process_command(UiCommand::LoadConversation(history));
    app.process_command(UiCommand::SetChannelPosts {
        channel_id: "c1".to_string(),
        posts: vec![ChannelPostView {
            seq: 1,
            author: ChannelPostAuthor::User,
            text: "@WK1 check the footer".to_string(),
            at_ms: DAY + 3_500,
            ..ChannelPostView::default()
        }],
    });
    app.select_pane_tab(PaneTab::Side(SidePanelTab::ChannelHistory));
    app
}

fn listed(app: &crate::App) -> Vec<(HistoryKind, String)> {
    app.channel_history()
        .into_iter()
        .map(|item| (item.kind, item.text))
        .collect()
}

#[test]
fn test_history_lists_what_the_user_typed_to_the_coordinator_among_the_posts() {
    let app = typed_app();
    assert_eq!(
        listed(&app),
        [
            (HistoryKind::Typed, "now ship it".to_string()),
            (HistoryKind::Post, "@WK1 check the footer".to_string()),
            (HistoryKind::Typed, LONG.to_string()),
        ],
        "newest first, and no mail from an agent"
    );
}

#[test]
fn test_a_long_typed_message_shows_its_summary_by_its_prompt_key() {
    let mut app = typed_app();
    app.process_command(UiCommand::SetPostSummaries {
        channel_id: "c1".to_string(),
        summaries: vec![(
            prompt_key(LONG),
            "Footer links open in new tabs".to_string(),
        )],
    });
    let summary = app.channel_history()[2].summary.clone();
    assert_eq!(summary.as_deref(), Some("Footer links open in new tabs"));
    let rows = render_rows_sized(&mut app, 160, 40).join("\n");
    assert!(rows.contains("Footer links open in new tabs"), "{rows}");
}

#[test]
fn test_enter_on_a_typed_message_jumps_the_chat_back_to_it() {
    let mut app = typed_app();
    app.focus = PanelFocus::Sandbox;
    let rows = render_rows_sized(&mut app, 160, 40);
    assert!(rows.iter().any(|row| row.contains("note 79")), "at its end");

    // The oldest, far back in the chat, is the last item.
    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    render_rows_sized(&mut app, 160, 40);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.chat_jumping());
    let rows = render_rows_sized(&mut app, 160, 40);
    let screen = rows.join("\n");
    assert!(!screen.contains("note 79"), "scrolled back:\n{screen}");
    assert!(screen.contains("Please make every link"), "{screen}");
    assert!(app.chat_flash().is_some(), "and lit");
}

#[test]
fn test_a_members_chat_keeps_the_typed_messages_listed() {
    let mut app = typed_app();
    app.process_command(UiCommand::SetActiveAgent("wk".to_string()));
    assert!(app.member_chat().is_some());
    assert_eq!(
        listed(&app).len(),
        3,
        "read from the coordinator's held chat"
    );
    app.focus = PanelFocus::Sandbox;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.chat_jumping(), "the jump waits for the channel's chat");
}
