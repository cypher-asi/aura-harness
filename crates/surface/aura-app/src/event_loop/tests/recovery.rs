//! Keeping agents at their work through trouble they can get past: a
//! revoked sign-in pauses them until a model call works again or the
//! user signs in; a turn that failed on a provider timeout while it owes
//! a report is retried, up to a cap; what no retry fixes fails at once.

use super::inbox::{assign_footer_asking, channel_with_member, code};
use super::*;
use crate::event_loop::recover::{self, tick_at};
use aura_exec_tools::todo::task_report_line;

const MINUTE_MS: u64 = 60_000;

fn now() -> u64 {
    aura_core_types::time::now_ms()
}

/// Hand the notice waiting in `notices` to the loop and finish the turn
/// it starts.
async fn run_next_notice(
    state: &mut LoopState<'_>,
    done: &mut mpsc::Receiver<WorkDone>,
    notices: &mut tokio::sync::mpsc::UnboundedReceiver<notices::Notice>,
) {
    notices::handle_notice(state, notices.try_recv().unwrap()).await;
    let finished = next_done(done).await;
    handle_work_done(state, finished).await;
}

/// Finish the `count` turns running, in whatever order they end.
async fn finish_turns(
    state: &mut LoopState<'_>,
    done: &mut mpsc::Receiver<WorkDone>,
    count: usize,
) {
    for _ in 0..count {
        let finished = next_done(done).await;
        handle_work_done(state, finished).await;
    }
}

fn row(state: &LoopState<'_>, agent: AgentId) -> aura_ui::events::AgentSummary {
    state.hub.member_rows().remove(&agent).unwrap()
}

/// `member` and `other` wait for a sign-in, paused, not failed: the
/// report `member` owes stays owed, nobody is told it failed, and the
/// stall sweep leaves it be.
async fn assert_paused(
    state: &mut LoopState<'_>,
    notices: &mut tokio::sync::mpsc::UnboundedReceiver<notices::Notice>,
    member: AgentId,
    other: AgentId,
) {
    let mut paused = state.hub.recovery.waiting_for_sign_in();
    paused.sort_by_key(|agent| agent.to_hex());
    let mut expected = vec![member, other];
    expected.sort_by_key(|agent| agent.to_hex());
    assert_eq!(paused, expected);
    assert!(row(state, member).waiting_for_sign_in);
    assert_eq!(row(state, member).error, None, "not shown failed");
    assert_eq!(state.hub.inbox.owed_by(member), 1);
    assert!(notices.try_recv().is_err(), "no report given up");
    for _ in 0..4 {
        crate::event_loop::stalls::sweep(state).await;
    }
    assert_eq!(state.hub.inbox.owed_by(member), 1);
}

#[tokio::test]
async fn a_revoked_sign_in_pauses_agents_and_a_call_that_works_resumes_them_all() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let (member, other) = (fixture.second, fixture.first);
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let task = "sign-in revoked: fix the footer links";
    assign_footer_asking(&fixture, coordinator, &code(&fixture, member), task).await;
    run_next_notice(&mut state, &mut done, &mut notices).await;
    turn(
        &mut state,
        &mut done,
        &links,
        other,
        "sign-in revoked, hello",
    )
    .await;

    assert_paused(&mut state, &mut notices, member, other).await;

    // The coordinator's model call works: both carry on by themselves.
    turn(
        &mut state,
        &mut done,
        &links,
        coordinator,
        "how is it going",
    )
    .await;
    assert!(state.hub.recovery.waiting_for_sign_in().is_empty());
    assert!(state.hub.is_running(member) && state.hub.is_running(other));
    finish_turns(&mut state, &mut done, 2).await;
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    let report = notices.try_recv().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(
        report.prompt.ends_with(&task_report_line("3", true)),
        "{}",
        report.prompt
    );
    let asked = user_texts(&state, member);
    assert!(
        asked
            .last()
            .is_some_and(|text| text.contains("Your model sign-in works again")),
        "{asked:?}"
    );
}

#[tokio::test]
async fn signing_in_resumes_a_paused_agent_and_a_second_refusal_waits_a_cooldown() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let task = "sign-in keeps failing: fix the footer links";
    assign_footer_asking(&fixture, coordinator, &code(&fixture, member), task).await;
    run_next_notice(&mut state, &mut done, &mut notices).await;
    assert!(state.hub.recovery.sign_in_wait(member).is_some());

    // The user signs in: it carries on at once, and is refused again.
    recover::signed_in(&mut state).await;
    assert!(state.hub.is_running(member), "resumed on sign-in");
    finish_turns(&mut state, &mut done, 1).await;
    assert!(
        state.hub.recovery.sign_in_wait(member).is_some(),
        "paused again"
    );

    // Straight after a resume, the next one waits two minutes.
    recover::signed_in(&mut state).await;
    assert!(!state.hub.is_running(member), "cooling down");
    tick_at(&mut state, now() + MINUTE_MS).await;
    assert!(!state.hub.is_running(member));
    tick_at(&mut state, now() + 3 * MINUTE_MS).await;
    assert!(state.hub.is_running(member), "resumed after the cooldown");
    finish_turns(&mut state, &mut done, 1).await;
    assert_eq!(
        state.hub.inbox.owed_by(member),
        1,
        "still owed, never given up"
    );
}

#[tokio::test]
async fn a_turn_that_timed_out_while_owing_a_report_is_retried_until_done() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let task = "provider timeout once: fix the footer links";
    assign_footer_asking(&fixture, coordinator, &code(&fixture, member), task).await;
    run_next_notice(&mut state, &mut done, &mut notices).await;

    assert!(state.hub.recovery.is_held(member));
    let why = row(&state, member).error.unwrap_or_default();
    assert!(why.contains("Retrying after an error (1/3)"), "{why}");
    assert_eq!(state.hub.inbox.owed_by(member), 1);
    assert!(notices.try_recv().is_err(), "no report given up");

    tick_at(&mut state, now() + 30_000).await;
    assert!(!state.hub.is_running(member), "not due yet");
    tick_at(&mut state, now() + MINUTE_MS + 1_000).await;
    assert!(state.hub.is_running(member), "retried");
    finish_turns(&mut state, &mut done, 1).await;

    assert_eq!(state.hub.inbox.owed_by(member), 0);
    let report = notices.try_recv().unwrap();
    assert!(
        report.prompt.ends_with(&task_report_line("3", true)),
        "{}",
        report.prompt
    );
    assert_eq!(state.hub.status.failure(member), None);
}

#[tokio::test]
async fn past_three_retries_a_turn_that_keeps_timing_out_fails_in_red() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let task = "provider keeps timing out: fix the footer links";
    assign_footer_asking(&fixture, coordinator, &code(&fixture, member), task).await;
    run_next_notice(&mut state, &mut done, &mut notices).await;
    for retry in 1..=3 {
        assert!(
            state.hub.recovery.is_held(member),
            "held before retry {retry}"
        );
        tick_at(&mut state, now() + 10 * MINUTE_MS).await;
        assert!(state.hub.is_running(member), "retry {retry}");
        finish_turns(&mut state, &mut done, 1).await;
    }

    assert!(!state.hub.recovery.is_held(member), "the cap is reached");
    let why = state.hub.status.failure(member).unwrap();
    assert!(
        why.contains("timed out") && !why.contains("Retrying"),
        "{why}"
    );
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    let report = notices.try_recv().unwrap();
    assert!(
        report.prompt.contains("Stopped before finishing"),
        "{}",
        report.prompt
    );
    assert!(report.prompt.ends_with(&task_report_line("3", false)));
}

#[tokio::test]
async fn an_error_no_retry_fixes_fails_the_turn_at_once() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    // A 400: the request itself is wrong.
    let task = "explode: fix the footer links";
    assign_footer_asking(&fixture, coordinator, &code(&fixture, member), task).await;
    run_next_notice(&mut state, &mut done, &mut notices).await;

    assert!(!state.hub.recovery.is_held(member));
    let why = state.hub.status.failure(member).unwrap();
    assert!(why.contains("the model fell over"), "{why}");
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    let report = notices.try_recv().unwrap();
    assert!(
        report.prompt.contains("Stopped before finishing"),
        "{}",
        report.prompt
    );
    tick_at(&mut state, now() + 60 * MINUTE_MS).await;
    assert!(!state.hub.is_running(member), "never retried");
}

#[tokio::test]
async fn a_cloud_turn_refused_its_sign_in_is_paused_and_resumed_not_failed() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    assign_footer_asking(&fixture, coordinator, &code(&fixture, member), "fix it").await;
    let _assignment = notices.try_recv().unwrap();

    subagents::lock(&state.hub.cloud_turns).insert(member);
    let outcome = cloud::TurnOutcome {
        failure: Some("API error (status 401): OAuth access token has been revoked".to_string()),
        messages: None,
        said: None,
    };
    cloud::handle_ended(&mut state, cloud::Ended::Turn(member, Some(outcome))).await;
    assert!(row(&state, member).waiting_for_sign_in);
    assert_eq!(state.hub.status.failure(member), None);
    assert_eq!(state.hub.inbox.owed_by(member), 1);
    assert!(notices.try_recv().is_err());

    recover::signed_in(&mut state).await;
    assert!(state.hub.is_running(member), "carries on");
    finish_turns(&mut state, &mut done, 1).await;
    assert_eq!(state.hub.inbox.owed_by(member), 0);
}

#[tokio::test]
async fn a_failed_cloud_start_keeps_its_work_and_tries_again() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    subagents::lock(&fixture.hub.cloud_members).insert(member);

    // Its machine is starting; the assignment waits in its queue.
    state.cloud.begin_test_move(member);
    state.hub.status.set_starting(member, true);
    assign_footer_asking(&fixture, coordinator, &code(&fixture, member), "fix it").await;
    notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;
    assert_eq!(state.queued.len(member), 1);

    // Its project didn't build: it waits for a fix, its work kept.
    state
        .cloud
        .fail_test_start_as(member, "it does not build", false);
    cloud::handle_ended(&mut state, cloud::Ended::Move(member)).await;
    assert!(state.hub.recovery.is_held(member));
    assert_eq!(state.queued.len(member), 1, "the work is kept");
    assert_eq!(state.hub.inbox.owed_by(member), 1);
    assert!(notices.try_recv().is_err(), "nobody told it failed");
    let why = state.hub.status.failure(member).unwrap();
    assert!(
        why.contains("Retrying the start in 10 min (try 1 of 6)"),
        "{why}"
    );

    // More work meanwhile waits with it, not turned away.
    assign_footer_asking(&fixture, coordinator, &code(&fixture, member), "and this").await;
    notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;
    assert_eq!(state.queued.len(member), 2);
    assert!(notices.try_recv().is_err());

    tick_at(&mut state, now() + 5 * MINUTE_MS).await;
    assert!(
        state.hub.recovery.is_held(member),
        "a fix gets time to land"
    );
    tick_at(&mut state, now() + 11 * MINUTE_MS).await;
    assert!(!state.hub.recovery.is_held(member), "tried again");
    // The fixture has no cloud to start on: that start fails as before.
    let why = state.hub.status.failure(member).unwrap();
    assert!(!why.contains("Retrying"), "{why}");
}

/// The model trouble the recovery tests ask for: a revoked sign-in or a
/// provider timeout on the turn that says so ("sign-in revoked",
/// "provider timeout once"), or on every turn after one that did
/// ("sign-in keeps failing", "provider keeps timing out").
pub(super) fn trouble_reply(request: &ModelRequest, last: Option<&str>) -> Option<ReasonerError> {
    // A failed turn leaves its prompt unanswered, so the prompt carrying
    // on joins it: the turn asked to carry on is the one that doesn't fail.
    let said = |words: &str| {
        last.is_some_and(|text| {
            text.contains(words) && !text.contains("Carry on where you left off")
        })
    };
    let ever = |words: &str| {
        request
            .messages
            .iter()
            .any(|m| m.text_content().contains(words))
    };
    let revoked = || ReasonerError::Api {
        status: 401,
        message: "OAuth access token has been revoked".to_string(),
    };
    let timed_out = || ReasonerError::Transient {
        status: 408,
        message: "Request timed out".to_string(),
        retry_after: None,
    };
    if said("sign-in revoked") || ever("sign-in keeps failing") {
        Some(revoked())
    } else if said("provider timeout once") || ever("provider keeps timing out") {
        Some(timed_out())
    } else {
        None
    }
}
