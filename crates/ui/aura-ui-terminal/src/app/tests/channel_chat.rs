use super::*;

#[test]
fn test_at_in_a_coordinator_chat_lists_members_before_files() {
    let (_dir, mut app) = app_in_channel("co");
    type_keys(&mut app, "ask @");
    let paths = mention_paths(&app);
    assert_eq!(paths[..3], ["CO1", "RE36", "SUB2"], "{paths:?}");
    assert!(paths[3..].contains(&"src/main.rs".to_string()), "{paths:?}");
    let roles: Vec<_> = app
        .mention_matches()
        .iter()
        .filter_map(|candidate| candidate.member.as_ref().map(|member| member.role))
        .collect();
    assert_eq!(roles, ["coordinator", "agent", "subagent"]);

    type_keys(&mut app, "rev");
    assert_eq!(mention_paths(&app), ["RE36"], "names match too");
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.input(), "ask @RE36 ");
    assert!(app.mention_matches().is_empty(), "inserting closes it");
}

#[test]
fn test_at_matches_member_codes_ignoring_case_and_tab_finishes_them() {
    let (_dir, mut app) = app_in_channel("co");
    type_keys(&mut app, "@sub");
    assert_eq!(mention_paths(&app), ["SUB2"]);
    app.handle_key(KeyEvent::from(KeyCode::Tab));
    assert_eq!(app.input(), "@SUB2 ");
}

#[test]
fn test_at_outside_a_coordinator_chat_lists_only_files() {
    let (_dir, mut app) = app_in_channel("re");
    type_keys(&mut app, "@");
    let matches = app.mention_matches();
    assert!(!matches.is_empty());
    assert!(
        matches.iter().all(|candidate| candidate.member.is_none()),
        "{matches:?}"
    );

    app.switch_app(AgentsTab::Projects);
    app.process_command(UiCommand::SetActiveAgent("co".to_string()));
    app.process_command(UiCommand::SetSandboxRoot(_dir.path().to_path_buf()));
    app.input.text.clear();
    app.input.cursor_pos = 0;
    type_keys(&mut app, "@RE");
    assert_eq!(mention_paths(&app), ["README.md"]);
}

#[test]
fn test_the_chat_bar_names_the_channel_and_has_no_tabs() {
    let (_dir, app) = app_in_channel("co");
    assert_eq!(
        app.channel_chat_title(),
        Some(("#zode".to_string(), String::new()))
    );
    assert!(app.chat_tab_layout(80).cells.is_empty());
}

#[test]
fn test_coordinator_chat_shows_the_channel_posts() {
    use crate::events::{ChannelPostAuthor, ChannelPostView};
    let (_dir, mut app) = app_in_channel("co");
    assert!(app.pane_is_empty());
    app.process_command(UiCommand::SetChannelPosts {
        channel_id: "c1".to_string(),
        posts: vec![ChannelPostView {
            seq: 1,
            author: ChannelPostAuthor::User,
            text: "@RE36 look".to_string(),
            at_ms: 1,
            ..ChannelPostView::default()
        }],
    });
    assert_eq!(app.channel_posts("c1").len(), 1);
    assert!(!app.pane_is_empty(), "the posts fill the chat");
    let ((_, posts), _, _) = app.chat_line_cache();
    assert_eq!(posts.len(), 1);

    app.process_command(UiCommand::SetActiveAgent("re".to_string()));
    let ((_, posts), _, _) = app.chat_line_cache();
    assert!(posts.is_empty(), "only the coordinator's chat shows them");
}

#[test]
fn test_messages_to_members_route_past_the_coordinator() {
    let (_dir, app) = app_in_channel("co");
    assert_eq!(
        app.routed_members("@re36, look and ask @SUB2 too @RE36"),
        Some(vec!["RE36".to_string(), "SUB2".to_string()])
    );
    assert_eq!(app.routed_members("@RE36 and @coordinator"), None);
    assert_eq!(app.routed_members("@RE36 and @CO1"), None);
    assert_eq!(app.routed_members("look at @src/main.rs"), None);
    let (_dir, app) = app_in_channel("re");
    assert_eq!(
        app.routed_members("@SUB2 hi"),
        None,
        "not a coordinator chat"
    );
}

#[test]
fn test_a_message_to_members_does_not_wait_on_the_coordinator() {
    let (_dir, mut app) = app_in_channel("co");
    type_keys(&mut app, "@RE36 look");
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_ne!(app.state(), AppState::Processing);
    assert!(app.pending.just_queued.is_none());
    assert!(
        app.notifications
            .current
            .as_ref()
            .is_some_and(|(text, _)| text == "Sent to RE36"),
        "{:?}",
        app.notifications.current
    );
}
