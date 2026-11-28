use super::channel_side_panel::app_in_two_channels;
use super::*;

/// [`app_in_channel`] with a live event channel, `rx`, to watch what the
/// app asks of the session from here on.
pub(super) fn app_in_channel_with_events(
    active: &str,
) -> (tempfile::TempDir, App, mpsc::Receiver<UiEvent>) {
    let (dir, mut app) = app_in_channel(active);
    let (tx, rx) = mpsc::channel(64);
    app.link.event_tx = Some(tx);
    (dir, app, rx)
}

/// The agent the last of the events waiting in `rx` asked to put on
/// screen (`FocusAgent` or `SelectAgent`), reading them all.
pub(super) fn switch_request(rx: &mut mpsc::Receiver<UiEvent>) -> Option<String> {
    let mut last = None;
    while let Ok(event) = rx.try_recv() {
        if let UiEvent::FocusAgent(id) | UiEvent::SelectAgent(id) = event {
            last = Some(id);
        }
    }
    last
}

/// Lay out the MEMBERS rows of [`app_in_channel`] at column 81, from row
/// 5 down, two rows each: `co` on rows 5 and 6, `re` on 7 and 8, the
/// subagent `sb` on 9 and 10.
fn lay_out_member_rows(app: &mut App) {
    app.set_channel_member_rows_area(Rect::new(81, 5, 28, 10));
}

#[test]
fn test_members_side_tab_moves_its_highlight_and_enter_opens_the_members_chat() {
    let (_dir, mut app, mut rx) = app_in_channel_with_events("co");
    app.focus = PanelFocus::Sandbox;
    assert_eq!(app.selected_channel_member(), 0);
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Down));
    }
    assert_eq!(app.selected_channel_member(), 2, "clamped to the members");
    app.handle_key(key(KeyCode::Home));
    assert_eq!(app.selected_channel_member(), 0);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.active_agent_id(),
        "co",
        "the coordinator's is the channel's"
    );
    assert_eq!(switch_request(&mut rx), None, "it is on screen already");
    assert!(app.member_chat().is_none());

    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.active_agent_id(), "re");
    assert_eq!(switch_request(&mut rx).as_deref(), Some("re"));
    assert!(app.channel_open(), "the channel stays open");
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    assert_eq!(app.member_chat().map(|m| m.id.as_str()), Some("re"));
    assert_eq!(app.selected_channel_member(), 1);
}

#[test]
fn test_clicking_a_member_opens_its_chat_and_the_coordinators_row_goes_back() {
    let (_dir, mut app, mut rx) = app_in_channel_with_events("co");
    lay_out_member_rows(&mut app);
    // On its name, past `re`'s fold marker.
    app.click_channel_member(88, 7);
    assert_eq!(switch_request(&mut rx).as_deref(), Some("re"));
    assert_eq!(app.member_chat().map(|m| m.id.as_str()), Some("re"));
    assert!(app.channel_open());

    // A subagent, nested under a member, opens the same way, from its
    // status row too.
    app.click_channel_member(82, 10);
    assert_eq!(switch_request(&mut rx).as_deref(), Some("sb"));
    assert_eq!(app.member_chat().map(|m| m.id.as_str()), Some("sb"));
    assert!(app.channel_open());

    app.click_channel_member(82, 6);
    assert_eq!(switch_request(&mut rx).as_deref(), Some("co"));
    assert!(app.member_chat().is_none(), "back on the channel's chat");
    assert!(app.channel_open());
}

#[test]
fn test_esc_on_an_empty_input_and_the_close_button_go_back_to_the_channel() {
    let (_dir, mut app, mut rx) = app_in_channel_with_events("co");
    app.open_channel_member(1);
    switch_request(&mut rx);

    // A draft keeps the member's chat: Esc is not for leaving it then.
    type_keys(&mut app, "hi");
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.active_agent_id(), "re", "a draft keeps it");
    assert_eq!(switch_request(&mut rx), None);
    app.handle_key(key(KeyCode::Backspace));
    app.handle_key(key(KeyCode::Backspace));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.active_agent_id(), "co");
    assert_eq!(switch_request(&mut rx).as_deref(), Some("co"));
    assert!(app.channel_open());

    // The desktop's close button closes a view over the chat first.
    app.open_channel_member(1);
    app.open_detail(DetailView::Graph);
    app.close_pane_view(Pane::Chat);
    assert!(app.detail_view().is_none());
    assert_eq!(app.active_agent_id(), "re", "one thing at a time");
    app.close_pane_view(Pane::Chat);
    assert_eq!(app.active_agent_id(), "co");
    assert!(app.channel_open());

    // With nothing to go back from, Esc does nothing of the kind.
    switch_request(&mut rx);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(switch_request(&mut rx), None);
}

#[test]
fn test_opening_another_channel_closes_a_member_chat() {
    let (_dir, mut app) = app_in_two_channels();
    let (tx, mut rx) = mpsc::channel(64);
    app.link.event_tx = Some(tx);
    app.open_channel_member(1);
    assert!(app.member_chat().is_some());

    app.open_channel_by_id("c2");
    assert_eq!(app.active_agent_id(), "dc", "its coordinator comes");
    assert!(app.member_chat().is_none());
    app.open_channel_by_id("c1");
    assert_eq!(app.active_agent_id(), "co", "not the member left there");
    assert!(app.member_chat().is_none());
    assert_eq!(switch_request(&mut rx).as_deref(), Some("co"));
}

#[test]
fn test_a_member_dropped_from_the_channel_gives_way_to_the_coordinator() {
    let (_dir, mut app, mut rx) = app_in_channel_with_events("co");
    app.open_channel_member(1);
    switch_request(&mut rx);

    // Another list that still has it keeps its chat.
    let channels = app.channels().to_vec();
    app.process_command(UiCommand::SetChannels(channels.clone()));
    assert_eq!(app.active_agent_id(), "re");
    assert_eq!(switch_request(&mut rx), None);

    let mut dropped = channels;
    dropped[0].members.retain(|member| member.id != "re");
    app.process_command(UiCommand::SetChannels(dropped));
    assert_eq!(app.active_agent_id(), "co");
    assert_eq!(switch_request(&mut rx).as_deref(), Some("co"));
    assert!(app.channel_open());
}

#[test]
fn test_a_member_chat_sends_to_the_member_and_names_it() {
    let (_dir, mut app, mut rx) = app_in_channel_with_events("co");
    assert_eq!(app.app_placeholder().as_deref(), Some("Message #zode"));
    app.open_channel_member(1);
    switch_request(&mut rx);
    assert_eq!(app.app_placeholder().as_deref(), Some("Message RE36"));
    assert_eq!(
        app.channel_chat_title().map(|(slug, _)| slug).as_deref(),
        Some("#zode"),
        "the channel's own title, which the member's covers"
    );
    assert_eq!(
        app.member_chat_title(),
        Some(("Reviewer".to_string(), "RE36 · agent · #zode".to_string()))
    );

    // The session confirming the switch changes nothing.
    app.process_command(UiCommand::SetActiveAgent("re".to_string()));
    // A mention is plain text to a member: no routing through the channel.
    type_keys(&mut app, "@SUB2 look");
    app.handle_key(key(KeyCode::Enter));
    let sent: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            UiEvent::UserMessage { agent_id, text, .. } => Some((agent_id, text)),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [("re".to_string(), "@SUB2 look".to_string())]);

    // A permanent agent's instance is named by its definition, and a
    // subagent's chat says what it is.
    let mut channels = app.channels().to_vec();
    channels[0].members[1].definition = Some("reviewer".to_string());
    app.process_command(UiCommand::SetChannels(channels));
    assert_eq!(
        app.member_chat_title().map(|(_, info)| info),
        Some("RE36 · reviewer · #zode".to_string())
    );
    app.open_channel_member(2);
    assert_eq!(
        app.member_chat_title(),
        Some(("Scout".to_string(), "SUB2 · subagent · #zode".to_string()))
    );
}

#[test]
fn test_a_member_put_on_screen_by_the_session_shows_its_chat_too() {
    let (_dir, mut app, mut rx) = app_in_channel_with_events("co");
    app.process_command(UiCommand::SetActiveAgent("sb".to_string()));
    assert!(app.channel_open());
    assert_eq!(app.member_chat().map(|m| m.id.as_str()), Some("sb"));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(switch_request(&mut rx).as_deref(), Some("co"));
}

#[test]
fn test_plan_in_the_channels_chat_asks_for_a_planner_with_the_request() {
    let (_dir, mut app, mut rx) = app_in_channel_with_events("co");
    let shown = app.messages().len();
    type_keys(&mut app, "/plan design the login, like");
    app.paste_clipboard(Ok(crate::clipboard::Clipboard::Image(
        b"\x89PNG\r\n\x1a\nimage".to_vec(),
    )));
    app.handle_key(key(KeyCode::Enter));

    let sent: Vec<UiEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(
        matches!(
            sent.as_slice(),
            [UiEvent::PlanInChannel { agent_id, text, images }]
                if agent_id == "co"
                    && text == "design the login, like [image 1]"
                    && images.len() == 1
        ),
        "only the planner is asked for: {sent:?}"
    );
    assert_eq!(
        app.messages().len(),
        shown,
        "the channel's chat shows none of it"
    );
    assert_eq!(app.attached_images(), 0);
    assert!(app.input().is_empty());

    // Bare, or as a command, it asks for one that waits for its request.
    submit(&mut app, "/plan ");
    app.handle_command("/plan");
    for _ in 0..2 {
        assert!(matches!(
            rx.try_recv(),
            Ok(UiEvent::PlanInChannel { text, images, .. }) if text.is_empty() && images.is_empty()
        ));
    }
    assert!(rx.try_recv().is_err());

    // The session lists the planner as a member, then puts it on screen:
    // its chat opens over the channel's, and Esc goes back.
    let mut channels = app.channels().to_vec();
    channels[0].members.push(crate::events::AgentSummary {
        id: "pl".to_string(),
        code: Some("ENG1".to_string()),
        name: "engineer #1".to_string(),
        ..crate::events::AgentSummary::default()
    });
    app.process_command(UiCommand::SetChannels(channels));
    app.process_command(UiCommand::SetActiveAgent("pl".to_string()));
    assert!(app.channel_open());
    assert_eq!(app.member_chat().map(|m| m.id.as_str()), Some("pl"));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(switch_request(&mut rx).as_deref(), Some("co"));
    assert!(app.channel_open());
}

#[test]
fn test_plan_in_a_members_chat_plans_in_that_member() {
    let (_dir, mut app, mut rx) = app_in_channel_with_events("re");
    submit(&mut app, "/plan tidy the footer");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SetMode { agent_id, mode: crate::PermissionMode::Plan }) if agent_id == "re"
    ));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { agent_id, text, .. })
            if agent_id == "re" && text == "tidy the footer"
    ));
    app.handle_command("/plan");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::TogglePlan(agent_id)) if agent_id == "re"));
}
