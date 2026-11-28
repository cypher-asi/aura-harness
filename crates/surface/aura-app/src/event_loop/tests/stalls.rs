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
    let (fixture, mut commands) = fixture();
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
    // Its chat says why, in red, and its row shows the error at once.
    let (errors, row) = shown(&mut commands, member);
    assert_eq!(errors, [crate::event_loop::stalls::stalled_note()]);
    assert_eq!(
        row,
        Some(Some(WENT_IDLE.to_string())),
        "the lists went out at once"
    );
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

/// The errors shown in `agent`'s chat so far, and the error its channel
/// member row carries in the last channel list sent: `None` when none
/// was sent since.
fn shown(
    commands: &mut mpsc::Receiver<UiCommand>,
    agent: AgentId,
) -> (Vec<String>, Option<Option<String>>) {
    let hex = agent.to_hex();
    let (mut errors, mut row) = (Vec::new(), None);
    for (to, command) in drain(commands) {
        match command {
            UiCommand::ShowError(why) if to.as_deref() == Some(hex.as_str()) => errors.push(why),
            UiCommand::SetChannels(channels) => {
                let member = channels
                    .iter()
                    .flat_map(|channel| &channel.members)
                    .find(|member| member.id == hex);
                row = Some(member.and_then(|member| member.error.clone()));
            }
            _ => {}
        }
    }
    (errors, row)
}

#[tokio::test]
async fn a_refused_cloud_start_says_why_in_red_and_fails_the_member() {
    let (fixture, mut commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let state = loop_state(&fixture, &mut b);
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let _assignment = notices.try_recv().unwrap();
    drain(&mut commands);

    let why = "Placed by Alice; sign in as Alice to manage it.";
    cloud::refused(&state, member, why.to_string()).await;
    let (errors, row) = shown(&mut commands, member);
    assert_eq!(errors, [why]);
    assert_eq!(
        row,
        Some(Some(why.to_string())),
        "the lists went out at once"
    );
    let report = notices.try_recv().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(report.prompt.contains(why), "{}", report.prompt);
}
