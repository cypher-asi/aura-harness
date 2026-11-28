//! Folding subagents on a channel's MEMBERS side tab: folded by default,
//! opened and folded by key and by a click on the marker, with the
//! highlight kept on a shown member.

use super::*;

/// `#zode` open on MEMBERS, focused: `co`, then `re` with subagents `s1`
/// (which has `s2`) and `s3`, then `ux`. Nothing unfolded.
fn folding_app() -> App {
    use crate::events::{AgentSummary, ChannelSummary};
    let member = |id: &str, depth: usize| AgentSummary {
        id: id.to_string(),
        name: id.to_string(),
        depth,
        ..AgentSummary::default()
    };
    let mut app = App::new();
    app.process_command(UiCommand::SetChannels(vec![ChannelSummary {
        id: "c1".to_string(),
        slug: "zode".to_string(),
        coordinator_id: "co".to_string(),
        members: vec![
            member("co", 0),
            member("re", 0),
            member("s1", 1),
            member("s2", 2),
            member("s3", 1),
            member("ux", 0),
        ],
        ..ChannelSummary::default()
    }]));
    app.open_channel_by_id("c1");
    app.focus = PanelFocus::Sandbox;
    app
}

/// The ids of the members shown, in order.
fn shown(app: &App) -> Vec<String> {
    let members = &app
        .open_channel()
        .map(|c| c.members.clone())
        .unwrap_or_default();
    app.shown_channel_members()
        .into_iter()
        .map(|i| members[i].id.clone())
        .collect()
}

/// The id of the highlighted member.
fn highlighted(app: &App) -> String {
    app.open_channel()
        .and_then(|c| c.members.get(app.selected_channel_member()))
        .map(|m| m.id.clone())
        .unwrap_or_default()
}

#[test]
fn test_subagents_start_folded_and_the_highlight_moves_over_shown_members() {
    let mut app = folding_app();
    assert_eq!(shown(&app), ["co", "re", "ux"]);
    assert_eq!(app.channel_member_fold(1), Some(true));
    assert_eq!(app.channel_member_fold(0), None, "no subagents to fold");
    for (code, id) in [
        (KeyCode::Down, "re"),
        (KeyCode::Down, "ux"),
        (KeyCode::Down, "ux"),
    ] {
        app.handle_key(key(code));
        assert_eq!(highlighted(&app), id);
    }
    app.handle_key(key(KeyCode::Up));
    assert_eq!(highlighted(&app), "re");
}

#[test]
fn test_space_and_right_open_a_member_and_left_folds_it() {
    let mut app = folding_app();
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(shown(&app), ["co", "re", "s1", "s3", "ux"]);
    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(shown(&app), ["co", "re", "ux"], "Space folds it again");

    app.handle_key(key(KeyCode::Right));
    assert_eq!(shown(&app), ["co", "re", "s1", "s3", "ux"]);
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Right));
    assert_eq!(shown(&app), ["co", "re", "s1", "s2", "s3", "ux"]);
    // Left on an open member folds it; on a folded one, goes up.
    app.handle_key(key(KeyCode::Left));
    assert_eq!(shown(&app), ["co", "re", "s1", "s3", "ux"]);
    assert_eq!(highlighted(&app), "s1");
    app.handle_key(key(KeyCode::Left));
    assert_eq!(highlighted(&app), "re");
    app.handle_key(key(KeyCode::Left));
    assert_eq!(shown(&app), ["co", "re", "ux"]);
}

#[test]
fn test_folding_over_the_highlight_parks_it_on_the_folded_member() {
    let mut app = folding_app();
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(highlighted(&app), "s3");
    app.toggle_channel_member(1);
    assert_eq!(highlighted(&app), "re");
    assert_eq!(shown(&app), ["co", "re", "ux"]);
}

#[test]
fn test_highlighting_a_hidden_member_from_elsewhere_unfolds_the_members_above_it() {
    let mut app = folding_app();
    // As its task in TASKS, or opening its chat, does.
    app.select_channel_member(3);
    assert_eq!(highlighted(&app), "s2");
    assert_eq!(shown(&app), ["co", "re", "s1", "s2", "s3", "ux"]);
    app.handle_key(key(KeyCode::End));
    assert_eq!(highlighted(&app), "ux");
}

#[test]
fn test_a_click_on_the_marker_folds_and_one_on_the_name_opens_the_chat() {
    let (tx, mut rx) = mpsc::channel(64);
    let mut app = folding_app();
    app.link.event_tx = Some(tx);
    // `co` on rows 5 and 6, `re` on 7 and 8, from column 81.
    app.set_channel_member_rows_area(Rect::new(81, 5, 28, 10));
    app.click_channel_member(82, 7);
    assert_eq!(shown(&app), ["co", "re", "s1", "s3", "ux"]);
    // Its status row is not the marker: it opens the member.
    app.click_channel_member(82, 8);
    assert_eq!(app.member_chat().map(|m| m.id.as_str()), Some("re"));
    app.click_channel_member(82, 7);
    assert_eq!(shown(&app), ["co", "re", "ux"]);
    // A member without subagents has no marker: the click opens it.
    let mut asked = None;
    app.click_channel_member(82, 9);
    while let Ok(event) = rx.try_recv() {
        if let UiEvent::FocusAgent(id) | UiEvent::SelectAgent(id) = event {
            asked = Some(id);
        }
    }
    assert_eq!(asked.as_deref(), Some("ux"));
}

#[test]
fn test_a_list_update_keeps_the_highlight_on_a_shown_member() {
    let mut app = folding_app();
    app.select_channel_member(3);
    app.toggle_channel_member(1);
    assert_eq!(highlighted(&app), "re");
    let channels = app.channels().to_vec();
    app.process_command(UiCommand::SetChannels(channels));
    assert_eq!(highlighted(&app), "re");
    assert_eq!(shown(&app), ["co", "re", "ux"]);
}
