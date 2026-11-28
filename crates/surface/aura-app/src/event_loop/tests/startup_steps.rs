//! Startup's steps once the loop takes UI events: their order, input
//! first, and resuming what the last run cut short through them.

use super::inbox::{assign_footer_asking, channel_with_member, code, SLOW_FOOTER};
use super::inbox_restart_resume::quit_mid_turn;
use super::*;
use crate::event_loop::inbox::{self as owed, Resume};
use crate::event_loop::startup::{Startup, Step};

#[tokio::test]
async fn the_steps_run_once_in_order_and_say_when_done() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    let mut startup = Startup::new(Resume::default());
    let _ = drain(&mut commands);
    let mut ran = Vec::new();
    while let Some(step) = startup.next_step() {
        ran.push(step);
        startup.run_next(&mut state).await;
    }
    assert_eq!(
        ran,
        [
            Step::Panels,
            Step::Cloud,
            Step::Resume,
            Step::Extras,
            Step::Done
        ]
    );
    let sent: Vec<UiCommand> = drain(&mut commands).into_iter().map(|(_, c)| c).collect();
    let done = sent
        .iter()
        .position(|c| matches!(c, UiCommand::StartupDone))
        .expect("startup says it is done");
    assert_eq!(done, sent.len() - 1, "last");
    assert!(
        sent[..done]
            .iter()
            .any(|c| matches!(c, UiCommand::SetSandboxRoot(_))),
        "the panels of the chat on screen came first"
    );
    assert!(!startup.may_run(&state));
}

#[tokio::test]
async fn a_waiting_ui_event_goes_before_a_step() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let (ui, events) = mpsc::channel(4);
    b.events = events;
    let state = loop_state(&fixture, &mut b);
    let startup = Startup::new(Resume::default());
    assert!(startup.may_run(&state), "nothing waits: the step runs");
    ui.send(UiEvent::RefreshAgents).await.unwrap();
    assert!(!startup.may_run(&state), "the user's event goes first");
}

#[tokio::test]
async fn a_cut_short_turn_resumes_through_the_resume_step() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    assign_footer_asking(&fixture, coordinator, &code(&fixture, member), SLOW_FOOTER).await;
    notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;
    state.hub.factory_flows.leave(member);
    owed::save_now(&state).await;
    quit_mid_turn(&mut state, &mut done, &mut notices, member).await;
    fixture.gate.add_permits(1);

    let restored = owed::restore_saved(&mut state).await;
    assert_eq!(state.hub.inbox.owed_by(member), 1, "owed before any step");
    let mut startup = Startup::new(restored);
    while let Some(step) = startup.next_step() {
        startup.run_next(&mut state).await;
        if step == Step::Cloud {
            assert!(!state.hub.is_running(member), "not resumed before its step");
        }
    }
    assert!(state.hub.is_running(member), "it carries on on its own");
    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, member);
}
