//! What an agent was asked, answered when its turn ends, or given up on
//! when its work stops.

use super::super::answers::{answer_seen, give_up_seen};
use super::super::hooks::peer_row;
use super::super::peers::circle_of;
use super::super::*;
use super::support::{code_of, new_channel, start, take_all, to_member};
use crate::event_loop::channels::create_channel;
use crate::event_loop::subagents::tests::{fixture, Fixture};
use tokio::sync::mpsc::UnboundedReceiver;

/// The conversation of an agent that saw `prompt` and said `said`.
fn conversation(prompt: &str, said: &str) -> Vec<Message> {
    vec![
        Message::user("earlier work"),
        Message::assistant("earlier answer"),
        Message::user(prompt),
        Message::assistant(said),
    ]
}

/// `first` having asked `second`, both members of `#site`, which parser
/// the tests use.
struct Asked {
    fixture: Fixture,
    notices: UnboundedReceiver<Notice>,
    first: AgentId,
    second: AgentId,
    /// `second`, as `first` reaches it
    peer: Peer,
    /// The request
    asked: Notice,
    /// How many answers `second` owed before
    asked_before: usize,
}

async fn ask_which_parser() -> Asked {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let first = start(&mut fixture, channel.coordinator, "reviewer").await;
    let second = start(&mut fixture, channel.coordinator, "tests").await;
    take_all(&mut notices);
    let asked_before = fixture.hub.inbox.owed_by(second);

    let circle = circle_of(&fixture.hub, first).unwrap();
    let peer = circle.find(&second.to_hex()).unwrap().clone();
    send(
        &fixture.hub,
        &circle,
        InboxKind::Request,
        &peer,
        Some("Parser"),
        "Which parser do the tests use?",
        None,
    )
    .unwrap();
    let [asked] = take_all(&mut notices).try_into().unwrap();
    Asked {
        fixture,
        notices,
        first,
        second,
        peer,
        asked,
        asked_before,
    }
}

#[tokio::test]
async fn a_request_says_how_it_is_answered_and_stays_owed_until_a_turn_sees_it() {
    let mut ask = ask_which_parser().await;
    let (hub, second) = (&ask.fixture.hub, ask.second);
    assert_eq!(ask.asked.inbox, Some(InboxKind::Request));
    assert!(ask.asked.prompt.ends_with(&format!(
        "End your turn with your answer: it goes back to {} on its own.",
        code_of(&ask.fixture, ask.first)
    )));
    assert_eq!(hub.inbox.owed_by(second), ask.asked_before + 1);

    // A turn that never saw it leaves it owed.
    answer_seen(hub, second, &conversation("something else", "Done."));
    assert_eq!(hub.inbox.owed_by(second), ask.asked_before + 1);
    assert!(take_all(&mut ask.notices).is_empty());
}

#[tokio::test]
async fn a_request_is_answered_with_the_last_words_of_the_turn_that_saw_it() {
    let mut ask = ask_which_parser().await;
    let (hub, first, second) = (&ask.fixture.hub, ask.first, ask.second);
    answer_seen(
        hub,
        second,
        &conversation(&ask.asked.prompt, "The LR one, in src/parse.rs."),
    );
    assert_eq!(hub.inbox.owed_by(second), ask.asked_before);
    let [reply] = take_all(&mut ask.notices).try_into().unwrap();
    assert_eq!(reply.to, first);
    assert_eq!(reply.inbox, Some(InboxKind::Reply));
    let code = code_of(&ask.fixture, second);
    assert!(
        reply
            .prompt
            .starts_with(&format!("{code} replied: Parser\n(inbox reply ")),
        "{}",
        reply.prompt
    );
    assert!(reply.prompt.ends_with("\n\nThe LR one, in src/parse.rs."));
    let row = peer_row(hub, &ask.peer);
    assert_eq!(row["last_report"]["text"], "The LR one, in src/parse.rs.");
    assert_eq!(row["last_report"]["finished"], true);

    // Replies are never answered.
    answer_seen(hub, first, &conversation(&reply.prompt, "Thanks."));
    assert!(take_all(&mut ask.notices).is_empty());
}

#[tokio::test]
async fn a_finished_assignment_reports_back_and_closes_the_coordinators_todo() {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let member = start(&mut fixture, coordinator, "reviewer").await;
    take_all(&mut notices);
    let args = AssignArgs {
        to: Some(code_of(&fixture, member)),
        agent: None,
        subject: "Footer".to_string(),
        prompt: "Fix the footer links.".to_string(),
        todo_id: Some("4".to_string()),
    };
    assign(&fixture.hub, coordinator, args).await.unwrap();
    let [assigned] = take_all(&mut notices).try_into().unwrap();

    answer_seen(
        &fixture.hub,
        member,
        &conversation(&assigned.prompt, "Fixed: the links were relative."),
    );
    let [report] = take_all(&mut notices).try_into().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(
        report.prompt.contains(" replied: Footer\n"),
        "{}",
        report.prompt
    );
    assert!(
        report.prompt.ends_with(&format!(
            "Fixed: the links were relative.\n{}",
            task_report_line("4", true)
        )),
        "{}",
        report.prompt
    );
    assert_eq!(
        fixture.hub.inbox.owed_by(member),
        1,
        "the kick-off it never saw is still owed"
    );
}

#[tokio::test]
async fn coordinators_owe_no_answers_and_are_told_how_to_give_one() {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let member = start(&mut fixture, coordinator, "reviewer").await;
    take_all(&mut notices);

    let circle = circle_of(&fixture.hub, member).unwrap();
    let lead = circle.find("coordinator").unwrap().clone();
    send(
        &fixture.hub,
        &circle,
        InboxKind::Request,
        &lead,
        Some("Scope"),
        "Should the footer cover the blog too?",
        None,
    )
    .unwrap();
    let [asked] = take_all(&mut notices).try_into().unwrap();
    assert_eq!(asked.to, coordinator);
    assert!(asked.prompt.ends_with(&format!(
        "Answer {} with `send_to_agent` if it needs an answer.",
        code_of(&fixture, member)
    )));
    assert_eq!(fixture.hub.inbox.owed_by(coordinator), 0);
    answer_seen(
        &fixture.hub,
        coordinator,
        &conversation(&asked.prompt, "Yes."),
    );
    assert!(take_all(&mut notices).is_empty());
}

/// In a new `#site`, a member started from `reviewer` that owes its
/// coordinator its kick-off and then the header, for todo 7. Returns the
/// coordinator, the member and the kick-off's notice.
async fn member_owing_two(
    fixture: &mut Fixture,
    notices: &mut UnboundedReceiver<Notice>,
) -> (AgentId, AgentId, Notice) {
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let member = start(fixture, coordinator, "reviewer").await;
    let [kickoff] = take_all(notices).try_into().unwrap();
    let code = code_of(fixture, member);
    assign(
        &fixture.hub,
        coordinator,
        AssignArgs {
            todo_id: Some("7".to_string()),
            ..to_member(&code, "Then the header.")
        },
    )
    .await
    .unwrap();
    take_all(notices);
    (coordinator, member, kickoff)
}

#[tokio::test]
async fn work_that_stops_tells_its_senders_no_answer_is_coming() {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let (coordinator, member, kickoff) = member_owing_two(&mut fixture, &mut notices).await;
    assert_eq!(fixture.hub.inbox.owed_by(member), 2);

    // With the conversation, only what it saw is dropped.
    give_up_seen(
        &fixture.hub,
        member,
        Some(&conversation(&kickoff.prompt, "")),
        "the model refused",
    );
    let [stopped] = take_all(&mut notices).try_into().unwrap();
    assert_eq!(stopped.to, coordinator);
    assert!(
        stopped
            .prompt
            .ends_with("\n\nStopped before finishing: the model refused"),
        "{}",
        stopped.prompt
    );
    assert_eq!(fixture.hub.inbox.owed_by(member), 1);

    // Without it, the oldest one ran.
    give_up_seen(&fixture.hub, member, None, "cancelled");
    let [stopped] = take_all(&mut notices).try_into().unwrap();
    assert!(
        stopped.prompt.ends_with(&format!(
            "Stopped before finishing: cancelled\n{}",
            task_report_line("7", false)
        )),
        "{}",
        stopped.prompt
    );
    assert_eq!(fixture.hub.inbox.owed_by(member), 0);
}
