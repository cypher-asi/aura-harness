//! Reports that never reached the coordinator, or came one message late:
//! each path, reproduced.
//!
//! An assignment was answered only when its message was still in the
//! conversation as the turn ended. Two things take it out:
//!
//! * A summary compaction replaces the older middle of a long
//!   conversation with one synthetic message. The message the turn
//!   started on is in that middle once the turn (or, in the software
//!   factory, the plan, build and ship turns after it) has run long.
//!   The report then stayed owed forever.
//! * A cloud turn's conversation is the checkpoint copied from its
//!   machine. When that copy fails, the checkpoint here is an earlier
//!   turn's. The report stayed owed, and the next turn whose copy held
//!   it answered it with that later turn's words: one message late.

use super::inbox::{assign_footer, channel_with_member, code};
use super::*;
use crate::event_loop::inbox::{self as owed};
use aura_model::Message;

/// What a summary compaction leaves of `messages`: the first message,
/// one synthetic summary for the middle and the newest `keep`, as
/// `aura_context_compaction::apply_message_summary` splices it.
fn compacted(messages: &[Message], keep: usize) -> Vec<Message> {
    let tail = messages.len().saturating_sub(keep).max(1);
    let mut out = vec![messages[0].clone()];
    out.push(Message::user(
        "[Summary of earlier conversation]\nThe agent was asked for the footer and worked on it.",
    ));
    out.extend_from_slice(&messages[tail..]);
    out
}

/// A long turn on `prompt`: many tool steps, then `said`.
fn long_turn(prompt: &str, said: &str) -> Vec<Message> {
    let mut messages = vec![
        Message::user("You are a member of #site."),
        Message::assistant("Ready."),
        Message::user(prompt),
    ];
    for step in 0..30 {
        messages.push(Message::assistant(format!("Step {step}: reading files.")));
        messages.push(Message::user(format!("tool output {step}")));
    }
    messages.push(Message::assistant(said));
    messages
}

#[tokio::test]
async fn a_report_whose_assignment_was_compacted_away_still_reaches_the_coordinator() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let assignment = notices.try_recv().unwrap();
    let asked = assignment.prompt.clone();

    // The turn starts on the assignment...
    notices::handle_notice(&mut state, assignment).await;
    let mut finished = next_done(&mut done).await;
    // ...and runs long enough that compaction summarizes its message.
    let session = finished.session.as_mut().expect("the member's session");
    session.messages = compacted(&long_turn(&asked, "Footer links fixed; PR #12 merged."), 6);
    assert!(
        !session
            .messages
            .iter()
            .any(|m| m.text_content().contains(&asked[..40])),
        "the assignment's message is gone"
    );
    handle_work_done(&mut state, finished).await;

    assert_eq!(state.hub.inbox.owed_by(member), 0, "nothing stays owed");
    let report = notices
        .try_recv()
        .expect("the report reaches the coordinator");
    assert_eq!(report.to, coordinator);
    assert!(
        report
            .prompt
            .contains("\n\nFooter links fixed; PR #12 merged.\n"),
        "{}",
        report.prompt
    );
    assert!(notices.try_recv().is_err(), "exactly one report");
}

#[tokio::test]
async fn a_failed_turn_whose_assignment_was_compacted_away_still_says_so() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let asked = notices.try_recv().unwrap().prompt;
    owed::take_up(&fixture.hub, member, &asked);

    let messages = compacted(&long_turn(&asked, "Half way through the footer."), 6);
    owed::give_up_seen(&fixture.hub, member, Some(&messages), "the model refused");

    let report = notices
        .try_recv()
        .expect("the coordinator hears it stopped");
    assert!(
        report.prompt.contains(
            "Stopped before finishing: the model refused\n\nWhat it said last:\nHalf way"
        ),
        "{}",
        report.prompt
    );
    assert_eq!(fixture.hub.inbox.owed_by(member), 0);
}

/// The next report, which must carry `said`.
fn assert_next_says(
    notices: &mut tokio::sync::mpsc::UnboundedReceiver<notices::Notice>,
    said: &str,
) {
    let report = notices.try_recv().expect("a report");
    assert!(words(&report).starts_with(said), "{}", report.prompt);
}

/// End `member`'s cloud turn, its copy holding `messages` and its stream
/// having said `said`.
async fn end_cloud_turn(
    state: &mut LoopState<'_>,
    member: AgentId,
    messages: Option<Vec<Message>>,
    said: &str,
) {
    subagents::lock(&state.hub.cloud_turns).insert(member);
    let outcome = cloud::TurnOutcome {
        failure: None,
        messages,
        said: Some(said.to_string()),
    };
    cloud::handle_ended(state, cloud::Ended::Turn(member, Some(outcome))).await;
}

/// The words a report carries, after its header.
fn words(report: &notices::Notice) -> &str {
    report
        .prompt
        .split_once("\n\n")
        .map_or("", |(_, rest)| rest)
}

/// Assign the member the footer again and start its turn on it, as a
/// cloud turn's start does. Returns the assignment's text.
async fn assign_and_start(
    fixture: &Fixture,
    notices: &mut tokio::sync::mpsc::UnboundedReceiver<notices::Notice>,
    coordinator: AgentId,
) -> String {
    let member = fixture.second;
    assign_footer(fixture, coordinator, &code(fixture, member)).await;
    let asked = notices.try_recv().unwrap().prompt;
    owed::take_up(&fixture.hub, member, &asked);
    asked
}

#[tokio::test]
async fn a_cloud_report_is_neither_lost_nor_late_when_its_copy_is_an_earlier_turns() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);

    // Turn 1, on A: copied fine.
    let a = assign_and_start(&fixture, &mut notices, coordinator).await;
    let copy_one = vec![Message::user(&a), Message::assistant("A: footer fixed.")];
    end_cloud_turn(
        &mut state,
        member,
        Some(copy_one.clone()),
        "A: footer fixed.",
    )
    .await;
    assert_next_says(&mut notices, "A: footer fixed.");

    // Turn 2, on B: its copy failed, so the checkpoint here is turn 1's.
    let b_asked = assign_and_start(&fixture, &mut notices, coordinator).await;
    end_cloud_turn(
        &mut state,
        member,
        Some(copy_one.clone()),
        "B: header fixed.",
    )
    .await;
    // B's report is not lost.
    assert_next_says(&mut notices, "B: header fixed.");

    // Turn 3, on C: copied fine, and holding B's message too.
    let c = assign_and_start(&fixture, &mut notices, coordinator).await;
    let mut copy_three = copy_one;
    copy_three.extend([
        Message::user(&b_asked),
        Message::assistant("B: header fixed."),
        Message::user(&c),
        Message::assistant("C: nav fixed."),
    ]);
    end_cloud_turn(&mut state, member, Some(copy_three), "C: nav fixed.").await;
    assert_next_says(&mut notices, "C: nav fixed.");
    assert!(
        notices.try_recv().is_err(),
        "B is not answered again, one message late"
    );
    assert_eq!(state.hub.inbox.owed_by(member), 0);
}

#[tokio::test]
async fn a_cloud_report_whose_copy_failed_outright_comes_from_its_stream() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let asked = notices.try_recv().unwrap().prompt;
    owed::take_up(&state.hub, member, &asked);

    end_cloud_turn(&mut state, member, None, "Footer links fixed.").await;
    let report = notices.try_recv().expect("answered from what streamed");
    assert!(
        words(&report).starts_with("Footer links fixed."),
        "{}",
        report.prompt
    );
    assert_eq!(state.hub.inbox.owed_by(member), 0);
}

#[tokio::test]
async fn a_request_no_turn_started_on_still_waits_for_its_own_turn() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let queued = notices.try_recv().unwrap().prompt;
    // Another turn ends while it waits in the queue: it answers nothing.
    let other = vec![
        Message::user("Run the tests."),
        Message::assistant("Green."),
    ];
    owed::answer_seen(&fixture.hub, member, &other);
    assert!(notices.try_recv().is_err(), "not answered by another turn");
    assert_eq!(fixture.hub.inbox.owed_by(member), 1);

    owed::take_up(&fixture.hub, member, &queued);
    let own = compacted(&long_turn(&queued, "Footer done."), 4);
    owed::answer_seen(&fixture.hub, member, &own);
    let report = notices.try_recv().unwrap();
    assert!(
        words(&report).starts_with("Footer done."),
        "{}",
        report.prompt
    );
}
