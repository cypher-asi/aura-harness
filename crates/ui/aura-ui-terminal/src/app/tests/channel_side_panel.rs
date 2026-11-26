use super::*;

/// [`app_in_channel`] on `co`, with `#docs` (`c2`) listed after `#zode`.
pub(super) fn app_in_two_channels() -> (tempfile::TempDir, App) {
    let (dir, mut app) = app_in_channel("co");
    let mut channels = app.channels().to_vec();
    channels.push(crate::events::ChannelSummary {
        id: "c2".to_string(),
        slug: "docs".to_string(),
        coordinator_id: "dc".to_string(),
        members: vec![agent("dc", "Docs lead")],
        ..crate::events::ChannelSummary::default()
    });
    app.process_command(UiCommand::SetChannels(channels));
    (dir, app)
}

#[test]
fn test_keys_and_opening_a_channel_take_the_focus_off_the_hidden_channel_panel() {
    let (_dir, mut app) = app_in_two_channels();
    app.toggle_side_column();
    assert!(!app.sandbox_panel_visible(), "the hamburger hid it");

    // Focus left on the hidden panel: its keys go to the chat instead.
    app.select_channel_member(1);
    let tab = app.side_tab();
    app.focus = PanelFocus::Sandbox;
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.focus(), PanelFocus::Chat);
    app.focus = PanelFocus::Sandbox;
    app.handle_key(key(KeyCode::Up));
    assert_eq!(
        app.selected_channel_member(),
        1,
        "no hidden highlight moves"
    );
    for code in [KeyCode::Right, KeyCode::Left] {
        app.focus = PanelFocus::Sandbox;
        app.handle_key(KeyEvent::new(code, KeyModifiers::SHIFT));
        assert_eq!(app.side_tab(), tab, "no hidden tab switches");
    }

    // Opening another channel, its panel hidden too, moves it at once.
    app.focus = PanelFocus::Sandbox;
    app.open_channel_by_id("c2");
    assert!(app.channel_open());
    assert_eq!(app.focus(), PanelFocus::Chat);
}

#[test]
fn test_tab_skips_the_channel_panel_the_hamburger_hid() {
    let (_dir, mut app) = app_in_channel("co");
    app.toggle_side_column();
    let mut reached = Vec::new();
    for _ in 0..4 {
        app.handle_key(key(KeyCode::Tab));
        reached.push(app.focus());
    }
    assert!(!reached.contains(&PanelFocus::Sandbox), "{reached:?}");
    assert!(reached.contains(&PanelFocus::Swarm), "{reached:?}");

    app.toggle_side_column();
    let reached: Vec<_> = (0..4)
        .map(|_| {
            app.handle_key(key(KeyCode::Tab));
            app.focus()
        })
        .collect();
    assert!(
        reached.contains(&PanelFocus::Sandbox),
        "shown again: {reached:?}"
    );
}

#[test]
fn test_a_hidden_channel_panel_stays_hidden_across_channels_and_apps() {
    let (_dir, mut app) = app_in_two_channels();
    let agent_layout = (app.panels.record_visible, app.panels.sandbox_visible);
    app.toggle_side_column();

    app.open_channel_by_id("c2");
    assert!(app.channel_open());
    assert!(
        !app.sandbox_panel_visible(),
        "another channel keeps it hidden"
    );

    // Among the agents the agent's own layout shows, untouched.
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Right));
    assert!(!app.channel_app());
    assert_eq!(
        (app.panels.record_visible, app.panels.sandbox_visible),
        agent_layout
    );
    assert_eq!(app.sandbox_panel_visible(), agent_layout.1);

    // Back in CHANNELS, the channel it was left on is still without it.
    app.handle_key(key(KeyCode::Left));
    assert!(app.channel_app());
    assert!(app.channel_open());
    assert!(!app.sandbox_panel_visible(), "still hidden on coming back");

    app.toggle_side_column();
    assert!(app.sandbox_panel_visible());
}
