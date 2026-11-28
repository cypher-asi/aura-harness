//! An inbox message that arrives while its agent's turn runs waits in the
//! agent's queue: it never goes into the running turn, which would carry
//! on past the reply it was writing, and it starts the next turn instead.

use super::inbox::{assign_footer_asking, channel_with_member, code, SLOW_FOOTER};
use super::*;
use crate::event_loop::inbox::{HubLink, PeerHooks};
use aura_exec_tools::{AgentControlHook, AgentMessage};

const RE_REVIEW: &str = "Re-review: two fixes on the footer.";

#[tokio::test]
async fn an_inbox_message_mid_turn_waits_for_the_turn_to_end() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let worker = code(&fixture, member);
    assign_footer_asking(&fixture, coordinator, &worker, SLOW_FOOTER).await;
    notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;
    assert!(state.hub.is_running(member), "the assignment is under way");

    // The coordinator writes to it while it works.
    write(&fixture, coordinator, &worker).await;
    let request = notices.try_recv().unwrap();
    notices::handle_notice(&mut state, request).await;
    assert!(
        state.hub.is_running(member),
        "its turn runs on, not stopped"
    );
    assert_eq!(
        state.queued.len(member),
        1,
        "the message waits in its queue"
    );

    // Its turn ends as it would have, without the message in it.
    fixture.gate.add_permits(2);
    let first = next_done(&mut done).await;
    assert_eq!(first.failure, None);
    let first_texts = texts_of(&first);
    assert!(
        !first_texts.iter().any(|text| text.contains(RE_REVIEW)),
        "{first_texts:?}"
    );
    assert!(first.unseen.is_empty());
    handle_work_done(&mut state, first).await;

    // Then the message starts the next turn.
    assert!(state.hub.is_running(member));
    assert_eq!(state.queued.len(member), 0);
    let second = next_done(&mut done).await;
    handle_work_done(&mut state, second).await;
    let asked = user_texts(&state, member);
    assert!(
        asked.last().is_some_and(|text| text.contains(RE_REVIEW)),
        "{asked:?}"
    );
}

/// `RE_REVIEW` from `from` to `to`, sent as its `send_to_agent` sends it.
async fn write(fixture: &Fixture, from: AgentId, to: &str) {
    let link = HubLink::default();
    link.set(&fixture.hub);
    PeerHooks::new(link, from, None)
        .deliver_message(AgentMessage::new(to, RE_REVIEW))
        .await
        .unwrap();
}

/// The text of each message of the conversation `done` left.
fn texts_of(done: &WorkDone) -> Vec<String> {
    done.session.as_ref().map_or_else(Vec::new, |session| {
        session
            .messages
            .iter()
            .map(aura_model::Message::text_content)
            .collect()
    })
}
