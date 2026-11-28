//! Talking to a channel's members from its coordinator's chat: routing
//! `@CODE` mentions, the replies posted back, and the coordinator's
//! digest of channel activity.

use super::*;
use aura_store_db::{ChannelPost, ChannelRecord, NewChannel, PostAuthor};

/// A channel `#site` with the fixture's second agent as a member;
/// returns the channel and the member's code.
async fn channel_with_member(fixture: &Fixture) -> (ChannelRecord, String) {
    let channel = channels::create_channel(
        &fixture.hub,
        NewChannel {
            slug: "site".to_string(),
            title: "Website relaunch".to_string(),
            description: "Ship the new marketing site.".to_string(),
            ..NewChannel::default()
        },
    )
    .await
    .unwrap();
    let member = lock_registry_agent(fixture, fixture.second);
    channels::join_parent_channel(&fixture.hub, channel.coordinator, &member).unwrap();
    let code = member.code.expect("registered agents have codes");
    (channel, code)
}

fn posts(fixture: &Fixture, channel: &ChannelRecord) -> Vec<ChannelPost> {
    fixture
        .hub
        .channels
        .as_ref()
        .unwrap()
        .posts(&channel.id, None)
        .unwrap()
}

/// The posts sent to `agent`'s chat.
fn posts_sent(commands: &[(Option<String>, UiCommand)], agent: AgentId) -> Vec<String> {
    let hex = agent.to_hex();
    commands
        .iter()
        .filter_map(|(to, command)| match command {
            UiCommand::ChannelPost { post, .. } if to.as_deref() == Some(hex.as_str()) => {
                Some(post.text.clone())
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_member_mention_reaches_only_that_member_and_its_reply_is_posted() {
    let (fixture, mut commands) = fixture();
    let (channel, code) = channel_with_member(&fixture).await;
    let (coordinator, member) = (channel.coordinator, fixture.second);
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let text = format!("@{} please review the footer", code.to_lowercase());
    handle_ui_event(&mut state, message(coordinator, &text, true), &links).await;
    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, member);
    handle_work_done(&mut state, finished).await;

    assert!(
        fixture.sent_by(coordinator).is_empty(),
        "no coordinator turn"
    );
    assert!(!state.hub.is_running(coordinator));
    let asked = user_texts(&state, member);
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(
        asked[0].starts_with("[#site channel] message from the user via the coordinator's chat:"),
        "{}",
        asked[0]
    );
    assert!(asked[0].contains("Website relaunch"), "{}", asked[0]);
    assert!(asked[0].ends_with(&text), "{}", asked[0]);

    let stored = posts(&fixture, &channel);
    assert_eq!(stored.len(), 2, "{stored:?}");
    assert_eq!(stored[0].author, PostAuthor::User);
    assert_eq!(stored[0].text, text);
    assert_eq!(stored[0].mentions, [member]);
    assert_eq!(stored[1].author, PostAuthor::Agent(member));
    assert_eq!(stored[1].reply_to, Some(stored[0].seq));
    assert_eq!(stored[1].text, "Done.");

    let sent = drain(&mut commands);
    assert_eq!(posts_sent(&sent, coordinator), [text.as_str(), "Done."]);
    assert_eq!(state.hub.channel_chat.pending_replies(), 0);
}

#[tokio::test]
async fn an_unknown_mention_leaves_the_message_to_the_coordinator() {
    let (fixture, _commands) = fixture();
    let (channel, _) = channel_with_member(&fixture).await;
    let coordinator = channel.coordinator;
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let text = "@foo what is the status?";
    assert!(!channels::route_user_message(&mut state, coordinator, text, &[], true).await);
    handle_ui_event(&mut state, message(coordinator, text, true), &links).await;
    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, coordinator);
    handle_work_done(&mut state, finished).await;

    assert!(!fixture.sent_by(coordinator).is_empty());
    assert!(fixture.sent_by(fixture.second).is_empty());
    assert!(posts(&fixture, &channel).is_empty());
}

#[tokio::test]
async fn the_coordinators_next_turn_starts_with_the_channel_activity_once() {
    let (fixture, _commands) = fixture();
    let (channel, code) = channel_with_member(&fixture).await;
    let (coordinator, member) = (channel.coordinator, fixture.second);
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let asked = format!("@{code} please review the footer");
    handle_ui_event(&mut state, message(coordinator, &asked, true), &links).await;
    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, member);
    handle_work_done(&mut state, finished).await;

    for text in ["what is the status?", "and now?"] {
        handle_ui_event(&mut state, message(coordinator, text, true), &links).await;
        let finished = next_done(&mut done).await;
        assert_eq!(finished.agent_id, coordinator);
        handle_work_done(&mut state, finished).await;
    }

    let prompts = user_texts(&state, coordinator);
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    let digest = &prompts[0];
    assert!(
        digest.starts_with("Channel activity since your last turn:\n"),
        "{digest}"
    );
    assert!(digest.contains(&format!("user: {asked}")), "{digest}");
    assert!(digest.contains(&format!("{code} ")), "{digest}");
    assert!(digest.contains(": Done."), "{digest}");
    assert!(digest.ends_with("\n\nwhat is the status?"), "{digest}");
    assert_eq!(prompts[1], "and now?", "nothing new since the last turn");
}

#[tokio::test]
async fn a_long_message_to_the_coordinator_is_summarized_for_history() {
    use aura_ui::events::prompt_key;
    let (fixture, mut commands) = fixture();
    let (channel, _) = channel_with_member(&fixture).await;
    let coordinator = channel.coordinator;
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    let session = crate::event_loop::agents::open_session(&state, coordinator).unwrap();
    state.idle.insert(coordinator, session);

    let text = "Please make every link in the footer open in a new tab, and check that \
                the privacy link points at the new policy page";
    assert!(!channels::route_user_message(&mut state, coordinator, text, &[], true).await);
    assert!(posts(&fixture, &channel).is_empty(), "it is no post");
    let wanted = prompt_key(text);
    let found = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(command) = commands.recv().await {
            if let UiCommand::SetPostSummaries {
                channel_id,
                summaries,
            } = command
            {
                if summaries.iter().any(|(key, _)| *key == wanted) {
                    return channel_id;
                }
            }
        }
        String::new()
    })
    .await;
    assert_eq!(
        found.ok(),
        Some(channel.id),
        "summarized under its prompt key"
    );
}
