//! Work a coordinator takes back before it started: withdrawn, or handed
//! to another member with its todo.

use super::super::answers::give_up_all;
use super::super::withdraw::{take_withdrawn, withdraw};
use super::super::*;
use super::support::{code_of, new_channel, start, take_all, to_member};
use crate::event_loop::channels::create_channel;
use crate::event_loop::subagents::tests::fixture;

#[tokio::test]
async fn withdrawn_assignments_are_dropped_unrun_and_never_reported() {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let member = start(&mut fixture, coordinator, "reviewer").await;
    let [kickoff] = take_all(&mut notices).try_into().unwrap();
    let hub = &fixture.hub;

    let code = code_of(&fixture, member);
    let said = assign(
        hub,
        coordinator,
        AssignArgs {
            withdraw: true,
            ..to_member(&code, "")
        },
    )
    .await
    .unwrap();
    assert!(
        said.starts_with("Took back 1 assignment(s) from "),
        "{said}"
    );
    assert_eq!(
        hub.inbox.owed_by(member),
        1,
        "owed until its queue reaches it"
    );
    assert_eq!(withdraw(hub, coordinator, member), 0, "already taken back");

    assert!(!take_withdrawn(hub, member, "some other work"));
    assert!(take_withdrawn(hub, member, &kickoff.prompt));
    assert_eq!(hub.inbox.owed_by(member), 0);
    assert!(!take_withdrawn(hub, member, &kickoff.prompt), "only once");

    // A withdrawn one is dropped without a word when the rest is given up.
    assign(hub, coordinator, to_member(&code, "Then the header."))
        .await
        .unwrap();
    take_all(&mut notices);
    withdraw(hub, coordinator, member);
    give_up_all(hub, member, "the agent was archived");
    assert!(take_all(&mut notices).is_empty());
    assert_eq!(hub.inbox.owed_by(member), 0);
}

#[tokio::test]
async fn withdrawing_needs_a_member_that_owes_something() {
    let mut fixture = fixture("unused");
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let member = start(&mut fixture, coordinator, "reviewer").await;
    let code = code_of(&fixture, member);
    let withdraw_from = |to: &str| AssignArgs {
        withdraw: true,
        ..to_member(to, "")
    };
    assert!(assign(&fixture.hub, coordinator, withdraw_from("NOPE1"))
        .await
        .unwrap_err()
        .contains("has no member `NOPE1`"));
    assign(&fixture.hub, coordinator, withdraw_from(&code))
        .await
        .unwrap();
    let said = assign(&fixture.hub, coordinator, withdraw_from(&code))
        .await
        .unwrap();
    assert!(said.contains("owes you no assignment"), "{said}");
}

#[tokio::test]
async fn handing_a_todo_to_another_member_takes_it_back_from_the_first() {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let first = start(&mut fixture, coordinator, "reviewer").await;
    let second = start(&mut fixture, coordinator, "reviewer").await;
    take_all(&mut notices);
    let with_todo = |agent| AssignArgs {
        todo_id: Some("9".to_string()),
        ..to_member(&code_of(&fixture, agent), "Fix the footer.")
    };

    assign(&fixture.hub, coordinator, with_todo(first))
        .await
        .unwrap();
    let [to_first] = take_all(&mut notices).try_into().unwrap();
    let said = assign(&fixture.hub, coordinator, with_todo(second))
        .await
        .unwrap();
    assert!(said.contains("Took the todo back from "), "{said}");
    let [to_second] = take_all(&mut notices).try_into().unwrap();

    let hub = &fixture.hub;
    assert!(take_withdrawn(hub, first, &to_first.prompt));
    assert!(
        !take_withdrawn(hub, second, &to_second.prompt),
        "the new one runs"
    );
    assert_eq!(hub.inbox.owed_by(first), 1, "its kick-off is still owed");
}
