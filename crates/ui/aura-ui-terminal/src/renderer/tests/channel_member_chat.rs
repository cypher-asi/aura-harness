use super::channels::channels_app;
use super::*;

/// [`channels_app`] with a live event channel, so agents come on screen,
/// `#zode`'s coordinator on it and its worker coded `WK7`.
fn channels_app_with_events() -> (
    crate::App,
    tokio::sync::mpsc::Receiver<crate::events::UiEvent>,
) {
    use crate::app::{AgentsTab, PaneTab};
    use crate::events::UiCommand;
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let mut app = crate::App::new().with_event_sender(tx);
    let mut channels = channels_app().channels().to_vec();
    channels[0].members[1].code = Some("WK7".to_string());
    app.process_command(UiCommand::SetChannels(channels));
    app.select_pane_tab(PaneTab::Agents(AgentsTab::Channels));
    app.process_command(UiCommand::SetActiveAgent("co".to_string()));
    assert_eq!(app.active_agent_id(), "co");
    (app, rx)
}

#[test]
fn test_a_member_chat_is_named_in_the_chat_bar_and_its_close_button_goes_back() {
    use crate::app::HoverTarget;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let mouse = |kind, column: u16, row: u16| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    };
    let (mut app, _rx) = channels_app_with_events();
    let rows = render_rows(&mut app, 140);
    let (row, column) = rows
        .iter()
        .enumerate()
        .find_map(|(y, text)| text.find("Worker").map(|x| (y, text[..x].chars().count())))
        .unwrap_or_else(|| panic!("no member row in:\n{}", rows.join("\n")));
    let at = |n: usize| u16::try_from(n).unwrap();
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        at(column),
        at(row),
    ));
    assert_eq!(app.active_agent_id(), "wk");

    let rows = render_rows(&mut app, 140);
    let screen = rows.join("\n");
    let bar = rows
        .iter()
        .position(|text| text.contains("WK7 · subagent · #zode"))
        .unwrap_or_else(|| panic!("no member bar in:\n{screen}"));
    assert!(rows[bar].contains("│ Worker"), "{:?}", rows[bar]);
    assert!(rows[bar].contains('✕'), "{:?}", rows[bar]);
    assert!(screen.contains("Message Worker"), "{screen}");

    let close = app.areas.detail_close.expect("the close button");
    app.handle_mouse(mouse(MouseEventKind::Moved, close.x, close.y));
    assert_eq!(app.hover(), Some(HoverTarget::CloseDetail));
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        close.x,
        close.y,
    ));
    assert_eq!(app.active_agent_id(), "co");
    assert!(app.channel_open());
    let rows = render_rows(&mut app, 140);
    let screen = rows.join("\n");
    assert!(!screen.contains("WK7 · subagent"), "{screen}");
    assert!(rows[bar].contains("Zode release"), "{:?}", rows[bar]);
    assert!(!rows[bar].contains('✕'), "{:?}", rows[bar]);
}

#[test]
fn test_a_hosted_member_chat_bar_is_closable() {
    use crate::app::{Pane, PaneTab, SidePanelTab};
    use crossterm::event::{KeyCode, KeyEvent};
    let chat_bar = |app: &crate::App| {
        app.pane_chrome()
            .iter()
            .find(|chrome| chrome.pane == Pane::Chat)
            .cloned()
            .expect("the chat's bar")
    };
    let (mut app, _rx) = channels_app_with_events();
    app.set_host_chrome(true);
    render_rows(&mut app, 140);
    assert!(!chat_bar(&app).closable);

    app.select_pane_tab(PaneTab::Side(SidePanelTab::ChannelMembers));
    app.handle_key(KeyEvent::from(KeyCode::Down));
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.active_agent_id(), "wk");
    render_rows(&mut app, 140);
    let bar = chat_bar(&app);
    assert!(bar.closable);
    assert_eq!(bar.title.as_deref(), Some("Worker"));
    assert_eq!(bar.info.as_deref(), Some("WK7 · subagent · #zode"));

    app.close_pane_view(Pane::Chat);
    assert_eq!(app.active_agent_id(), "co");
    render_rows(&mut app, 140);
    let bar = chat_bar(&app);
    assert!(!bar.closable);
    assert_eq!(bar.title.as_deref(), Some("#zode"));
}

/// [`channels_app_with_events`] with the worker's chat opened from the
/// focused MEMBERS tab, and the receiver that keeps the session's end
/// open.
fn worker_chat_app() -> (
    crate::App,
    tokio::sync::mpsc::Receiver<crate::events::UiEvent>,
) {
    use crate::app::{PaneTab, SidePanelTab};
    use crossterm::event::{KeyCode, KeyEvent};
    let (mut app, rx) = channels_app_with_events();
    app.select_pane_tab(PaneTab::Side(SidePanelTab::ChannelMembers));
    app.handle_key(KeyEvent::from(KeyCode::Down));
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.active_agent_id(), "wk");
    (app, rx)
}

/// Draw `app`, check the chat bar names the worker and ends in its close
/// button, and click that button.
fn click_the_member_chat_close_button(app: &mut crate::App) {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let rows = render_rows(app, 140);
    let bar = rows
        .iter()
        .find(|text| text.contains("WK7 · subagent · #zode"))
        .unwrap_or_else(|| panic!("no member bar in:\n{}", rows.join("\n")));
    assert!(bar.contains('✕'), "{bar:?}");
    let close = app.areas.detail_close.expect("the close button");
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: close.x,
        row: close.y,
        modifiers: KeyModifiers::empty(),
    });
}

#[test]
fn test_a_member_chat_with_the_channel_panel_hidden_keeps_its_close_button() {
    use crate::app::PanelFocus;
    let (mut app, _rx) = worker_chat_app();
    assert_eq!(app.focus(), PanelFocus::Sandbox, "opened from MEMBERS");
    app.toggle_side_column();
    assert_eq!(app.focus(), PanelFocus::Chat, "off the hidden panel");

    click_the_member_chat_close_button(&mut app);
    assert_eq!(app.active_agent_id(), "co");
    assert!(app.channel_open());
    assert_eq!(app.focus(), PanelFocus::Chat);
    assert!(!app.sandbox_panel_visible(), "the panel stays hidden");
    let rows = render_rows(&mut app, 140);
    assert_eq!(app.areas.sandbox, None, "{}", rows.join("\n"));
}

#[test]
fn test_an_expanded_member_chat_keeps_its_close_button_and_stays_expanded() {
    let (mut app, _rx) = worker_chat_app();
    app.toggle_chat_expanded();
    click_the_member_chat_close_button(&mut app);
    assert_eq!(app.active_agent_id(), "co");
    assert!(app.channel_open());
    assert!(
        app.chat_expanded(),
        "the channel's chat comes back expanded"
    );
    let rows = render_rows(&mut app, 140);
    assert!(
        rows.iter().any(|row| row.contains("Zode release")),
        "{}",
        rows.join("\n")
    );
}
