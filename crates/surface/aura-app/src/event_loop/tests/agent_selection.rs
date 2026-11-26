//! Selecting and focusing agents and channels.

use super::*;

#[tokio::test]
async fn focusing_an_agent_makes_it_active_without_reloading_its_chat() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let (first, second) = (fixture.first, fixture.second);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, UiEvent::SelectAgent(second.to_hex()), &links).await;
    let selected = drain(&mut commands);
    assert!(
        selected
            .iter()
            .any(|(_, c)| matches!(c, UiCommand::LoadConversation(_))),
        "selecting an idle agent loads its chat"
    );

    handle_ui_event(&mut state, UiEvent::FocusAgent(first.to_hex()), &links).await;
    assert_eq!(state.active, first);
    settle(&mut state).await;
    let focused = drain(&mut commands);
    assert!(matches!(
        focused.first(),
        Some((None, UiCommand::SetActiveAgent(id))) if *id == first.to_hex()
    ));
    assert!(
        focused
            .iter()
            .any(|(_, c)| matches!(c, UiCommand::SetRecords(_))),
        "the Record panel follows the focused agent"
    );
    assert!(
        !focused
            .iter()
            .any(|(_, c)| matches!(c, UiCommand::LoadConversation(_))),
        "the view that asked already has the chat"
    );
}

#[tokio::test]
async fn a_selected_agent_gets_its_chat_before_its_records() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(
        &mut state,
        UiEvent::SelectAgent(fixture.second.to_hex()),
        &links,
    )
    .await;
    settle(&mut state).await;
    let sent = drain(&mut commands);
    let at = |wanted: fn(&UiCommand) -> bool| sent.iter().position(|(_, c)| wanted(c));
    let chat = at(|c| matches!(c, UiCommand::LoadConversation(_))).expect("chat sent");
    let records = at(|c| matches!(c, UiCommand::SetRecords(_))).expect("records sent");
    assert!(chat < records, "the chat the UI waits on comes first");
}

#[tokio::test]
async fn selecting_the_active_agent_again_sends_its_chat() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let second = fixture.second;
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, UiEvent::SelectAgent(second.to_hex()), &links).await;
    let _ = drain(&mut commands);

    // A UI that showed another agent meanwhile, without the session
    // following, asks for this one's chat again.
    handle_ui_event(&mut state, UiEvent::SelectAgent(second.to_hex()), &links).await;
    let again = drain(&mut commands);
    assert!(again
        .iter()
        .any(|(_, c)| matches!(c, UiCommand::LoadConversation(_))));

    handle_ui_event(&mut state, UiEvent::FocusAgent(second.to_hex()), &links).await;
    let focused = drain(&mut commands);
    assert!(!focused
        .iter()
        .any(|(_, c)| matches!(c, UiCommand::LoadConversation(_))));
}

#[tokio::test]
async fn a_channel_opens_with_its_posts_before_its_chat_and_loads_it_once() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);

    let create = UiEvent::CreateChannel {
        slug: "zode".to_string(),
        title: "Zode".to_string(),
        description: String::new(),
        project_id: None,
    };
    handle_ui_event(&mut state, create, &links).await;
    let sent = drain(&mut commands);
    let at = |wanted: fn(&UiCommand) -> bool| sent.iter().position(|(_, c)| wanted(c));
    let posts = at(|c| matches!(c, UiCommand::SetChannelPosts { .. })).expect("posts sent");
    let chat = at(|c| matches!(c, UiCommand::LoadConversation(_))).expect("chat sent");
    assert!(
        posts < chat,
        "the chat fades in with its posts, not before them"
    );
    let channel_id = sent
        .iter()
        .find_map(|(_, c)| match c {
            UiCommand::SetChannels(list) => list.first().map(|channel| channel.id.clone()),
            _ => None,
        })
        .expect("the channel is listed");
    let coordinator = state.active;

    // A UI that switched to the coordinator itself asked for it first; the
    // channel opening after does not load its chat again.
    handle_ui_event(
        &mut state,
        UiEvent::FocusAgent(coordinator.to_hex()),
        &links,
    )
    .await;
    let _ = drain(&mut commands);
    handle_ui_event(&mut state, UiEvent::OpenChannel(channel_id), &links).await;
    let opened = drain(&mut commands);
    assert_eq!(state.active, coordinator);
    assert!(
        !opened
            .iter()
            .any(|(_, c)| matches!(c, UiCommand::LoadConversation(_))),
        "{opened:?}"
    );
}
