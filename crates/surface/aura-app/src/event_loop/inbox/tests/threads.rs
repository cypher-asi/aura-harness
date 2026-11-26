//! The threads two agents write in, and how much they carry.

use super::super::answers::capped;
use super::super::messages::thread_id;
use super::super::peers::circle_of;
use super::super::*;
use super::support::{code_of, new_channel, start, take_all};
use crate::event_loop::channels::create_channel;
use crate::event_loop::subagents::tests::fixture;

#[tokio::test]
async fn a_thread_carries_a_bounded_number_of_requests() {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let first = start(&mut fixture, channel.coordinator, "reviewer").await;
    let second = start(&mut fixture, channel.coordinator, "tests").await;
    let circle = circle_of(&fixture.hub, first).unwrap();
    let peer = circle.find(&second.to_hex()).unwrap().clone();
    let ask = |subject: &str| {
        send(
            &fixture.hub,
            &circle,
            InboxKind::Request,
            &peer,
            Some(subject),
            "And now?",
            None,
        )
    };

    for _ in 0..THREAD_REQUESTS {
        ask("Parser").unwrap();
    }
    let error = ask("Parser").unwrap_err();
    assert!(error.contains("has carried 8 messages"), "{error}");
    assert!(
        error.contains(&format!(
            "take what is still open to {} in a short report",
            code_of(&fixture, channel.coordinator)
        )),
        "members are pointed at their coordinator: {error}"
    );
    ask("Another subject").unwrap();
    assert_eq!(
        take_all(&mut notices).len(),
        2 + THREAD_REQUESTS as usize + 1
    );
}

#[test]
fn threads_are_the_same_whoever_writes_and_long_text_is_marked_when_cut() {
    let (a, b) = (AgentId::new([1; 32]), AgentId::new([2; 32]));
    assert_eq!(thread_id(a, b, "Parser").len(), "inbox:".len() + 16);
    assert_eq!(thread_id(a, b, "Parser"), thread_id(b, a, "Parser"));
    assert_ne!(thread_id(a, b, "Parser"), thread_id(a, b, "Lexer"));
    assert_eq!(capped("  short  ", 10), "short");
    assert_eq!(capped("ééééé", 3), "ééé… [cut at 3 characters]");
}
