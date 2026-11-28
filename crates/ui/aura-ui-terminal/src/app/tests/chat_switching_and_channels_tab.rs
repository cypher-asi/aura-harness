use super::*;

#[test]
fn a_failed_switch_goes_back_to_the_agent_the_session_kept() {
    let (mut app, mut rx) = app_with_agents();
    app.handle_command("/agent 2");
    let _ = rx.try_recv();
    assert_eq!(app.active_agent_id(), "bb");

    app.process_command(UiCommand::SwitchFailed("bb".into()));
    assert_eq!(app.active_agent_id(), "aa");
    assert!(!app.chat_loading());

    // bb's history was never loaded, so it is asked for again.
    app.handle_command("/agent 2");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
}

#[test]
fn chat_ready_ends_the_wait_for_a_live_chat() {
    let (mut app, mut rx) = app_with_agents();
    app.handle_command("/agent 2");
    let _ = rx.try_recv();
    app.process_command(UiCommand::SetActiveAgent("bb".into()));
    assert!(app.chat_loading());

    app.process_command(UiCommand::ForAgent {
        agent_id: "bb".into(),
        command: Box::new(UiCommand::ChatReady),
    });
    assert!(!app.chat_loading());
    assert!(
        app.chat_fading().is_none(),
        "a quick answer shows it at once"
    );
}

#[test]
fn an_unlisted_agent_waits_for_the_session() {
    let (mut app, mut rx) = app_with_agents();
    app.request_agent_switch("elsewhere".into());
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "elsewhere"));
    assert_eq!(app.active_agent_id(), "aa");
    assert!(!app.chat_loading());
}

#[test]
fn a_loading_chat_draws_no_welcome_and_says_it_loads_after_a_moment() {
    let (mut app, mut rx) = app_with_agents();
    app.handle_command("/agent 2");
    let _ = rx.try_recv();

    let text = drawn_text(&mut app);
    assert!(!text.contains("Type a message"), "{text}");
    assert!(!text.contains("Loading chat"), "a quick load says nothing");

    app.switching.chat_loading_since =
        Instant::now().checked_sub(super::super::run_log::LOADING_DELAY);
    let text = drawn_text(&mut app);
    assert!(text.contains("Loading chat"), "{text}");

    load_chat_of(&mut app, "bb", "bb's history");
    let text = drawn_text(&mut app);
    assert!(text.contains("bb's history"), "{text}");
}

#[test]
fn test_agents_tabs_put_channels_first_and_open_on_projects() {
    let labels = AgentsTab::ALL.map(AgentsTab::label);
    assert_eq!(
        labels,
        ["CHANNELS", "PROJECTS", "AGENTS", "BENCH", "ARCHIVE"]
    );
    assert_eq!(App::new().agents_tab(), AgentsTab::Projects);
}

#[test]
fn test_coming_to_channels_opens_one_and_leaving_the_tab_closes_it() {
    let (mut app, mut rx) = app_with_channel();

    app.handle_key(key(KeyCode::Enter));
    assert!(app.channel_open());
    assert!(rx.try_recv().is_err(), "it is open already");
    assert_eq!(app.agents_tab(), AgentsTab::Channels, "the list stays");

    // Esc and Backspace leave the channel open: the tab is how to leave.
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Backspace));
    assert!(app.channel_open());

    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    assert!(!app.channel_open());
    assert!(matches!(rx.try_recv(), Ok(UiEvent::CloseChannel)));
    assert!(
        rx.try_recv().is_err(),
        "the coordinator was on screen before"
    );

    // Coming back opens the channel it was left on.
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    assert!(app.channel_open());
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenChannel(id)) if id == "c1"));
}

/// The app with `#zode` listed, coordinated by `bb`, so `aa`, on screen,
/// is not a member; the agents panel focused on PROJECTS, Left away from
/// CHANNELS.
fn app_with_others_channel() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app_with_agents();
    app.process_command(UiCommand::SetChannels(vec![zode("bb")]));
    app.focus = PanelFocus::Swarm;
    (app, rx)
}

#[test]
fn test_leaving_channels_gives_back_the_agent_there_was_before() {
    let (mut app, mut rx) = app_with_others_channel();
    app.handle_key(key(KeyCode::Left));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenChannel(_))));
    // The session confirms the coordinator on screen.
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    assert!(app.channel_open());

    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::CloseChannel)));
    assert!(
        matches!(
            rx.try_recv(),
            Ok(UiEvent::SelectAgent(id) | UiEvent::FocusAgent(id)) if id == "aa"
        ),
        "aa, on screen before the channel, comes back"
    );
}

#[test]
fn test_opening_a_channel_shows_its_coordinator_at_once_not_the_agent_before() {
    let (mut app, mut rx) = app_with_others_channel();
    load_chat_of(&mut app, "aa", "aa's history");
    assert!(app.messages().iter().any(|m| m.content() == "aa's history"));

    app.handle_key(key(KeyCode::Left));
    // Asked for before the channel, so the session has it active by then
    // and does not load it twice.
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenChannel(id)) if id == "c1"));
    assert_eq!(app.active_agent_id(), "bb", "before the session answers");
    assert!(app.chat_loading(), "waits blank for its history");
    let text = drawn_text(&mut app);
    assert!(!text.contains("aa's history"), "{text}");

    // An answer the session gave for aa before it got the request is old.
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    assert_eq!(app.active_agent_id(), "bb");
    assert!(app.channel_open());

    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    load_chat_of(&mut app, "bb", "coordinator history");
    assert!(!app.chat_loading());
    assert!(drawn_text(&mut app).contains("coordinator history"));
}

#[test]
fn test_reopening_a_channel_shows_the_coordinators_held_chat_at_once() {
    let (mut app, mut rx) = app_with_others_channel();
    app.handle_key(key(KeyCode::Left));
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    load_chat_of(&mut app, "bb", "coordinator history");
    app.handle_key(key(KeyCode::Right));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    while rx.try_recv().is_ok() {}

    app.handle_key(key(KeyCode::Left));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::FocusAgent(id)) if id == "bb"));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenChannel(id)) if id == "c1"));
    assert!(!app.chat_loading(), "the held chat needs no reload");
    assert!(drawn_text(&mut app).contains("coordinator history"));
}

#[test]
fn test_a_non_member_on_screen_takes_the_user_back_to_projects() {
    let (mut app, mut rx) = app_with_others_channel();
    app.handle_key(key(KeyCode::Left));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(_))));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenChannel(_))));
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    assert!(app.channel_open());

    // Once the coordinator came, another agent put on screen is the user
    // going to it.
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    assert!(!app.channel_open());
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::CloseChannel)));
    assert!(rx.try_recv().is_err(), "nothing is asked for in its place");
}

/// Shift+arrows go between the channel's five tabs only, from MEMBERS,
/// ending on INFO.
fn shift_arrows_cycle_the_channel_tabs(app: &mut App) {
    let shift = |code| KeyEvent::new(code, KeyModifiers::SHIFT);
    for (code, tab) in [
        (KeyCode::Right, SidePanelTab::ChannelInfo),
        (KeyCode::Right, SidePanelTab::ChannelQueue),
        (KeyCode::Right, SidePanelTab::Tasks),
        (KeyCode::Right, SidePanelTab::ChannelHistory),
        (KeyCode::Right, SidePanelTab::ChannelMembers),
        (KeyCode::Left, SidePanelTab::ChannelHistory),
        (KeyCode::Left, SidePanelTab::Tasks),
        (KeyCode::Left, SidePanelTab::ChannelQueue),
        (KeyCode::Left, SidePanelTab::ChannelInfo),
    ] {
        assert!(app.handle_side_tab_key(shift(code)));
        assert_eq!(app.side_tab(), tab);
    }
}

/// [`app_with_others_channel`] come to its channel from a layout of the
/// user's own: Record shown, the side panel hidden and not maximized, on
/// TRACE.
fn channel_over_own_layout() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app_with_others_channel();
    app.panels.record_visible = true;
    app.panels.side_expanded = false;
    app.panels.sandbox_visible = false;
    app.side.tab = SidePanelTab::Trace;
    app.handle_key(key(KeyCode::Left));
    assert!(app.channel_open());
    (app, rx)
}

#[test]
fn test_an_open_channel_fills_the_right_column_with_its_tabs() {
    let (mut app, _rx) = channel_over_own_layout();
    assert!(!app.record_panel_visible(), "no Kernel panel in a channel");
    assert!(
        app.sandbox_panel_visible(),
        "shown though the agent's side panel is hidden"
    );
    assert!(app.side_maximized());
    assert_eq!(app.side_tab(), SidePanelTab::ChannelMembers);
    assert_eq!(
        SidePanelTab::tabs(true),
        [
            SidePanelTab::ChannelMembers,
            SidePanelTab::ChannelInfo,
            SidePanelTab::ChannelQueue,
            SidePanelTab::Tasks,
            SidePanelTab::ChannelHistory,
        ]
    );

    shift_arrows_cycle_the_channel_tabs(&mut app);
}

#[test]
fn test_an_open_channel_keeps_the_agent_panels_away_and_gives_back_the_users() {
    let (mut app, _rx) = channel_over_own_layout();
    // The agent panels stay away, and the user's flags are untouched.
    for command in ["/kernel", "/files", "/inbox", "/members", "/policies"] {
        app.handle_command(command);
        assert!(!app.record_panel_visible(), "{command}");
        assert!(app.sandbox_panel_visible(), "{command}");
    }
    app.toggle_side_expanded();
    app.show_side_tab(SidePanelTab::Trace);
    assert_eq!(app.side_tab(), SidePanelTab::ChannelMembers);
    assert!(
        (
            app.panels.record_visible,
            app.panels.sandbox_visible,
            app.panels.side_expanded
        ) == (true, false, false),
        "the user's flags are untouched"
    );

    // TASKS is the agent app's tab too, yet leaving from it gives back
    // the tab there was before the channel.
    app.show_side_tab(SidePanelTab::Tasks);
    assert_eq!(app.side_tab(), SidePanelTab::Tasks);
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Right));
    assert!(!app.channel_open());
    assert!(app.record_panel_visible());
    assert!(!app.sandbox_panel_visible());
    assert!(!app.side_expanded());
    assert!(!app.side_maximized());
    assert_eq!(app.side_tab(), SidePanelTab::Trace);
}

#[test]
fn test_a_channel_dropped_from_the_list_gives_back_the_side_tab() {
    let (mut app, _rx) = app_with_others_channel();
    app.side.tab = SidePanelTab::Tasks;
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.side_tab(), SidePanelTab::ChannelMembers);

    app.process_command(UiCommand::SetChannels(Vec::new()));
    assert!(!app.channel_open());
    assert_eq!(app.side_tab(), SidePanelTab::Tasks);
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    assert!(
        !app.sandbox_panel_visible(),
        "nothing to show beside the list"
    );
}

#[test]
fn test_channel_side_tabs_are_listed_and_reached_only_in_a_channel() {
    let channel_tabs = [
        SidePanelTab::ChannelMembers,
        SidePanelTab::ChannelInfo,
        SidePanelTab::ChannelQueue,
        SidePanelTab::ChannelHistory,
    ];
    for tab in channel_tabs {
        assert!(!SidePanelTab::tabs(false).contains(&tab));
    }
    for &tab in SidePanelTab::tabs(false) {
        assert!(!tab.next(false).is_channel_tab(), "{tab:?}");
        assert!(!tab.prev(false).is_channel_tab(), "{tab:?}");
    }
    assert_eq!(
        SidePanelTab::ChannelInfo.next(true),
        SidePanelTab::ChannelQueue
    );
    assert_eq!(SidePanelTab::ChannelQueue.next(true), SidePanelTab::Tasks);
    assert_eq!(SidePanelTab::Tasks.next(true), SidePanelTab::ChannelHistory);
    assert_eq!(
        SidePanelTab::ChannelHistory.next(true),
        SidePanelTab::ChannelMembers
    );
    assert_eq!(
        SidePanelTab::Sandbox.next(true),
        SidePanelTab::ChannelMembers
    );

    let mut app = App::new();
    app.show_side_tab(SidePanelTab::Trace);
    app.show_side_tab(SidePanelTab::ChannelMembers);
    app.show_side_tab(SidePanelTab::ChannelInfo);
    app.show_side_tab(SidePanelTab::ChannelQueue);
    app.show_side_tab(SidePanelTab::ChannelHistory);
    assert_eq!(app.side_tab(), SidePanelTab::Trace, "no channel is open");
    let mut shown = Vec::new();
    for _ in 0..SidePanelTab::tabs(false).len() * 2 {
        app.handle_side_tab_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        shown.push(app.side_tab());
        app.handle_side_tab_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        app.handle_side_tab_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        shown.push(app.side_tab());
    }
    assert!(!shown.iter().any(|tab| tab.is_channel_tab()), "{shown:?}");
}

#[test]
fn test_a_channel_has_only_the_chat_tab() {
    let (mut app, _rx) = app_with_channel();
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.chat_tab(), ChatTab::Chat);

    for tab in ChatTab::ALL {
        app.show_chat_tab(tab);
        assert_eq!(app.chat_tab(), ChatTab::Chat, "{tab:?} is an agent tab");
        app.restore_chat_tab(tab);
        assert_eq!(app.chat_tab(), ChatTab::Chat, "{tab:?} put back");
    }
    app.focus = PanelFocus::Chat;
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    app.handle_command("/log");
    assert_eq!(app.chat_tab(), ChatTab::Chat);
    assert!(
        app.chat_tab_layout(80).cells.is_empty(),
        "the bar has no tabs"
    );

    // Back among the agents the tabs come back.
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Right));
    app.show_chat_tab(ChatTab::Terminal);
    assert_eq!(app.chat_tab(), ChatTab::Terminal);
    assert!(!app.chat_tab_layout(80).cells.is_empty());
}

#[test]
fn test_opening_a_channel_puts_the_chat_on_chat() {
    let (mut app, _rx) = app_with_agents();
    app.show_chat_tab(ChatTab::Terminal);
    app.process_command(UiCommand::SetChannels(vec![
        crate::events::ChannelSummary {
            id: "c1".to_string(),
            slug: "zode".to_string(),
            coordinator_id: "aa".to_string(),
            members: vec![agent("aa", "Coordinator")],
            ..crate::events::ChannelSummary::default()
        },
    ]));
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    assert_eq!(
        app.chat_tab(),
        ChatTab::Chat,
        "the channel app has only CHAT"
    );
}

#[test]
fn test_channels_with_nothing_open_hide_the_right_column_and_refuse_to_send() {
    let (mut app, mut rx) = app_with_agents();
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    assert!(app.app_empty(), "no channel to open");
    assert!(!app.record_panel_visible());
    assert!(!app.sandbox_panel_visible());
    assert_eq!(app.app_placeholder().as_deref(), Some("Pick a channel"));

    app.input.text = "hello".to_string();
    app.input.cursor_pos = app.input.text.len();
    app.submit_input();
    assert_eq!(app.input(), "hello", "the draft is kept");
    assert!(rx.try_recv().is_err(), "nothing is sent");
    assert!(app
        .notifications
        .current
        .as_ref()
        .is_some_and(|(text, _)| text.contains("Pick a channel")));

    // The first channel listed opens as it comes.
    app.process_command(UiCommand::SetChannels(vec![
        crate::events::ChannelSummary {
            id: "c1".to_string(),
            slug: "zode".to_string(),
            coordinator_id: "aa".to_string(),
            members: vec![agent("aa", "Coordinator")],
            ..crate::events::ChannelSummary::default()
        },
    ]));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenChannel(id)) if id == "c1"));
    assert_eq!(app.app_placeholder().as_deref(), Some("Message #zode"));
    assert!(app.sandbox_panel_visible());
}
