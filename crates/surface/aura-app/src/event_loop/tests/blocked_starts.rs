//! A member whose start failed for good (its project doesn't build) is
//! not started again for each message sent to it: the message is turned
//! away at once. The user's own message retries it, and so does work
//! after a while.

use super::inbox::{assign_footer, channel_with_member, code};
use super::*;
use aura_exec_tools::todo::task_report_line;

/// The compile error a start failed with, for good.
const COMPILE: &str = "`cargo test --workspace --no-run` on aura-proj-575d-build-fa8472 exited \
                       with 101: error[E0063]: missing field `run_started`";

/// A cloud channel's member whose start failed with `why`, transient or
/// not, and the loop that saw it: the fixture, its coordinator and the
/// member, and the notices its senders get.
async fn failed_member(fixture: &Fixture, state: &mut LoopState<'_>, transient: bool) -> AgentId {
    let member = fixture.second;
    subagents::lock(&fixture.hub.cloud_members).insert(member);
    state.cloud.begin_test_move(member);
    state.hub.status.begin_start(member);
    state.cloud.fail_test_start_as(member, COMPILE, transient);
    cloud::handle_ended(state, cloud::Ended::Move(member)).await;
    let failure = state.hub.status.failure(member).unwrap();
    assert!(failure.contains("run_started"), "{failure}");
    member
}

#[tokio::test]
async fn work_for_a_member_whose_project_does_not_build_is_turned_away_without_a_start() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    let member = failed_member(&fixture, &mut state, false).await;

    for _ in 0..3 {
        assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
        notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;
        assert!(!state.hub.status.is_starting(member), "no new start");
        let report = notices.try_recv().unwrap();
        assert!(
            report.prompt.contains("Not started again"),
            "{}",
            report.prompt
        );
        assert!(report.prompt.ends_with(&task_report_line("3", false)));
    }
    // Still failed for its own reason: no new start replaced it with
    // another (the fixture has no boxd, so a start would fail otherwise).
    let failure = state.hub.status.failure(member).unwrap();
    assert!(failure.contains("run_started"), "{failure}");
    assert_eq!(state.hub.inbox.owed_by(member), 0);
}

#[tokio::test]
async fn a_start_its_machine_failed_is_tried_again_by_the_next_work() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    let member = failed_member(&fixture, &mut state, true).await;

    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;
    // It started again, and failed on its own reason: no boxd here.
    let failure = state.hub.status.failure(member).unwrap();
    assert!(!failure.contains("run_started"), "{failure}");
    let report = notices.try_recv().unwrap();
    assert!(
        !report.prompt.contains("Not started again"),
        "{}",
        report.prompt
    );
}

#[tokio::test]
async fn the_users_own_message_retries_a_member_that_failed_for_good() {
    let (fixture, _commands) = fixture();
    channel_with_member(&fixture).await;
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let member = failed_member(&fixture, &mut state, false).await;

    handle_ui_event(
        &mut state,
        message(member, "try again please", true),
        &links,
    )
    .await;
    let failure = state.hub.status.failure(member).unwrap();
    assert!(
        !failure.contains("run_started"),
        "a new start ran: {failure}"
    );
    assert!(!failure.contains("Not started again"), "{failure}");
}

#[tokio::test]
async fn a_blocked_start_lets_one_try_through_after_a_while() {
    let (fixture, _commands) = fixture();
    let status = &fixture.hub.status;
    let agent = AgentId::generate();
    assert_eq!(
        status.blocked_start(agent, 1_000, 600_000),
        None,
        "not blocked"
    );
    status.block_start(agent, "it doesn't build", 1_000);
    assert_eq!(
        status.blocked_start(agent, 500_000, 600_000).as_deref(),
        Some("it doesn't build")
    );
    assert_eq!(
        status.blocked_start(agent, 601_000, 600_000),
        None,
        "one try"
    );
    assert!(
        status.blocked_start(agent, 602_000, 600_000).is_some(),
        "the clock restarts"
    );
    status.unblock_start(agent);
    assert_eq!(status.blocked_start(agent, 603_000, 600_000), None);
}
