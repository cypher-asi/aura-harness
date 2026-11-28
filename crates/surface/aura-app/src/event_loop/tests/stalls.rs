//! The stall sweep and work that can't start: a member that owes a
//! report with nothing running is asked to carry on once, then reported;
//! one whose work can't start is reported at once.

use super::inbox::{assign_footer, channel_with_member, code};
use super::*;
use crate::event_loop::stalls::{sweep, WENT_IDLE};
use aura_exec_tools::todo::task_report_line;

#[tokio::test]
async fn the_sweep_asks_an_idle_member_owing_a_report_to_carry_on_once() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    // Its assignment never started, as one lost on its way does.
    let _lost = notices.try_recv().unwrap();
    assert_eq!(state.hub.inbox.owed_by(member), 1);

    sweep(&mut state).await;
    assert!(!state.hub.is_running(member), "seen idle once: nothing yet");
    sweep(&mut state).await;
    assert!(state.hub.is_running(member), "asked to carry on");
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    // The retry names what it owes, so its report answers it.
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    let report = notices.try_recv().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(
        report.prompt.ends_with(&task_report_line("3", true)),
        "{}",
        report.prompt
    );
}

#[tokio::test]
async fn the_sweep_reports_a_member_still_idle_after_its_retry_and_leaves_parked_ones_be() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let _lost = notices.try_recv().unwrap();

    // The user stopped it to carry on: its report waits on the user.
    state.hub.inbox.park(member);
    for _ in 0..4 {
        sweep(&mut state).await;
    }
    assert!(!state.hub.is_running(member));
    assert_eq!(state.hub.inbox.owed_by(member), 1, "parked: left alone");

    // Its next work started and went idle again, after a retry.
    state.hub.inbox.unpark(member);
    state.hub.standing.retried(member);
    sweep(&mut state).await;
    assert_eq!(state.hub.inbox.owed_by(member), 1);
    sweep(&mut state).await;
    assert!(!state.hub.is_running(member), "not retried twice");
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    let report = notices.try_recv().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(
        report
            .prompt
            .contains(&format!("Stopped before finishing: {WENT_IDLE}")),
        "{}",
        report.prompt
    );
    assert!(report.prompt.ends_with(&task_report_line("3", false)));
    let view = crate::event_loop::standing::state_of(&state.hub, member);
    assert_eq!(view.state, "stalled");
}

#[tokio::test]
async fn work_that_cannot_start_reports_what_its_agent_owes() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let _assignment = notices.try_recv().unwrap();

    // Other work waiting for it answers instead.
    state.queued.push_back(
        member,
        delivery::prompt_work(SteerSource::User, "Next".into(), false),
    );
    crate::event_loop::inbox::give_up_unstarted(&state, member, "its local model is not running");
    assert_eq!(state.hub.inbox.owed_by(member), 1);
    assert!(notices.try_recv().is_err());

    state.queued.pop_front(member);
    crate::event_loop::inbox::give_up_unstarted(&state, member, "its local model is not running");
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    let report = notices.try_recv().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(report
        .prompt
        .contains("Stopped before finishing: its local model is not running"));
    let view = crate::event_loop::standing::state_of(&state.hub, member);
    assert_eq!(view.state, "failed");
    assert_eq!(view.why.as_deref(), Some("its local model is not running"));
}
