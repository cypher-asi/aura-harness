//! A channel member's chat where other things want the same key, click
//! or switch: menus and the queue taking Esc, a view over the chat before
//! the close button, a switch the session refuses, a notification to an
//! agent outside the channel, and a session that cannot be told.

use super::channel_member_chat::{app_in_channel_with_events, switch_request};
use super::*;

/// [`app_in_channel_with_events`] with `RE36`'s chat open, the switch to
/// it read off `rx`.
fn app_in_member_chat() -> (tempfile::TempDir, App, mpsc::Receiver<UiEvent>) {
    let (dir, mut app, mut rx) = app_in_channel_with_events("co");
    app.open_channel_member(1);
    assert_eq!(switch_request(&mut rx).as_deref(), Some("re"));
    (dir, app, rx)
}

/// Type `text` into the member chat of `app`, check it opens a menu
/// (`menu_open`), and that Esc closes the menu but keeps the member chat;
/// then clear the input and check Esc goes back to the channel.
fn esc_closes_the_menu_then_the_chat(
    app: &mut App,
    rx: &mut mpsc::Receiver<UiEvent>,
    text: &str,
    menu_open: fn(&App) -> bool,
) {
    app.open_channel_member(1);
    switch_request(rx);
    type_keys(app, text);
    assert!(menu_open(app), "{text:?} opens its menu");
    app.handle_key(key(KeyCode::Esc));
    assert!(!menu_open(app), "Esc closed the {text:?} menu");
    assert_eq!(app.active_agent_id(), "re", "the member chat stays");
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.active_agent_id(), "re", "{text:?} is still typed");
    assert_eq!(switch_request(rx), None);
    for _ in text.chars() {
        app.handle_key(key(KeyCode::Backspace));
    }
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.active_agent_id(), "co", "with the input empty");
    assert_eq!(switch_request(rx).as_deref(), Some("co"));
}

/// The @ and slash menus open only with text in the input, and text
/// alone keeps the member chat on Esc. So this checks what the user
/// sees, not the order Esc is offered in: Esc closes the menu and leaves
/// the chat, and goes back to the channel once the input is empty.
#[test]
fn test_esc_closes_a_menu_and_leaves_a_member_chat_only_once_the_input_is_empty() {
    let (dir, mut app, mut rx) = app_in_channel_with_events("co");
    // The session gives the member's workspace, which the @ menu lists.
    app.open_channel_member(1);
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    let mention_open: fn(&App) -> bool = |app| !app.mention_matches().is_empty();
    let slash_open: fn(&App) -> bool = |app| !app.slash_matches().is_empty();
    esc_closes_the_menu_then_the_chat(&mut app, &mut rx, "@", mention_open);
    esc_closes_the_menu_then_the_chat(&mut app, &mut rx, "/", slash_open);
}

/// A queue item is selected with the input empty, so here the order is
/// what decides: the queue takes Esc before the member chat does.
#[test]
fn test_a_selected_queue_item_takes_esc_before_a_member_chat() {
    let (_dir, mut app, mut rx) = app_in_member_chat();
    app.process_command(UiCommand::SetQueue(vec![crate::events::QueuedItemView {
        id: 1,
        text: "later".to_string(),
        editable: true,
    }]));
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
    assert_eq!(app.queue_selected(), Some(0));
    assert!(app.input().is_empty());
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.queue_selected(), None);
    assert_eq!(app.active_agent_id(), "re", "the queue had the key");
    assert_eq!(switch_request(&mut rx), None);

    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.active_agent_id(), "co", "then Esc goes back");
    assert_eq!(switch_request(&mut rx).as_deref(), Some("co"));
}

#[test]
fn test_the_close_button_closes_a_view_over_a_member_chat_first() {
    let (_dir, mut app, mut rx) = app_in_member_chat();
    app.open_detail(DetailView::Graph);
    app.areas.chat = Some(Rect::new(20, 0, 60, 20));
    app.areas.detail_close = Some(Rect::new(76, 1, 2, 1));
    let click = |app: &mut App| {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 76,
            row: 1,
            modifiers: KeyModifiers::empty(),
        });
    };

    click(&mut app);
    assert!(app.detail_view().is_none(), "the view closes first");
    assert_eq!(app.active_agent_id(), "re");
    assert_eq!(switch_request(&mut rx), None);

    click(&mut app);
    assert_eq!(app.active_agent_id(), "co");
    assert_eq!(switch_request(&mut rx).as_deref(), Some("co"));
    assert!(app.channel_open());
}

#[test]
fn test_a_member_the_session_cannot_switch_to_gives_back_the_channel() {
    let (_dir, mut app, _rx) = app_in_member_chat();
    app.process_command(UiCommand::SwitchFailed("re".to_string()));
    assert_eq!(app.active_agent_id(), "co", "the agent the session kept");
    assert!(app.channel_open());
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    assert!(app.member_chat().is_none());
}

#[test]
fn test_a_notification_from_a_member_chat_to_an_outsider_leaves_the_channel() {
    let (_dir, mut app, mut rx) = app_in_member_chat();
    app.process_command(UiCommand::SetActiveAgent("re".to_string()));
    app.process_command(UiCommand::SetActiveAgent("zz".to_string()));
    assert_eq!(app.active_agent_id(), "zz");
    assert!(!app.channel_open());
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    assert!(app.member_chat().is_none());
    let closed = std::iter::from_fn(|| rx.try_recv().ok())
        .any(|event| matches!(event, UiEvent::CloseChannel));
    assert!(closed, "the session hears the channel closed");
}

#[test]
fn test_a_dropped_member_gives_way_even_when_the_session_cannot_be_told() {
    let (_dir, mut app, rx) = app_in_channel_with_events("co");
    // The coordinator's chat still waits for its history as the member's
    // opens; that wait goes off screen with it.
    app.switching.chat_loaded = false;
    app.switching.chat_loading_since = Some(std::time::Instant::now());
    app.open_channel_member(1);
    assert_eq!(app.active_agent_id(), "re");
    drop(rx);
    let mut channels = app.channels().to_vec();
    channels[0].members.retain(|member| member.id != "re");
    app.process_command(UiCommand::SetChannels(channels));
    assert_eq!(app.active_agent_id(), "co", "not an outsider under #zode");
    assert!(app.channel_open());
    assert_eq!(app.member_chat_title(), None);
    assert!(!app.chat_loading(), "no history is coming to wait for");

    // The session still had the member active; its late word for it puts
    // it on screen, as no member any more: the channel closes.
    app.process_command(UiCommand::SetActiveAgent("re".to_string()));
    assert_eq!(app.active_agent_id(), "re");
    assert!(!app.channel_open(), "never under #zode");
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
}
