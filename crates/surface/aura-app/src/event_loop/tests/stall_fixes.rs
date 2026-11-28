//! PR #74's review: the stall sweep and the inbox around it. Queued
//! inbox work the user removes or edits away is settled, not brought
//! back; work taken back is not asked for; a failed member leaves the
//! software factory; a turn waiting on its busy machine is left alone;
//! and peers see each queue's length however it changed.

use super::inbox::{assign_footer, assign_footer_asking, channel_with_member, code, SLOW_FOOTER};
use super::*;
use crate::event_loop::inbox::{assign, AssignArgs, REMOVED_BY_USER};
use crate::event_loop::stalls::sweep;
use crate::event_loop::standing::state_of;

/// A member busy on a slow assignment, a second assignment queued
/// behind it: the coordinator, the member and the queued entry's id.
async fn member_with_queued_assignment(
    fixture: &Fixture,
    state: &mut LoopState<'_>,
    notices: &mut mpsc::UnboundedReceiver<crate::event_loop::notices::Notice>,
) -> (AgentId, AgentId, u64) {
    let coordinator = channel_with_member(fixture).await;
    let member = fixture.second;
    let worker = code(fixture, member);
    assign_footer_asking(fixture, coordinator, &worker, SLOW_FOOTER).await;
    notices::handle_notice(state, notices.try_recv().unwrap()).await;
    assert!(state.hub.is_working(member));
    assign_footer(fixture, coordinator, &worker).await;
    notices::handle_notice(state, notices.try_recv().unwrap()).await;
    assert_eq!(state.hub.inbox.owed_by(member), 2, "the second one waits");
    let id = state.queued.views(member)[0].id;
    (coordinator, member, id)
}

#[tokio::test]
async fn removing_a_queued_assignment_settles_it_and_the_sweep_never_brings_it_back() {
    let (fixture, _commands) = fixture();
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let (coordinator, member, id) =
        member_with_queued_assignment(&fixture, &mut state, &mut notices).await;

    let remove = UiEvent::RemoveQueued {
        agent_id: member.to_hex(),
        id,
    };
    handle_ui_event(&mut state, remove, &links).await;
    assert_eq!(state.hub.inbox.owed_by(member), 1, "only the running one");
    let settled = notices.try_recv().unwrap();
    assert_eq!(settled.to, coordinator);
    assert!(
        settled.prompt.contains(REMOVED_BY_USER),
        "{}",
        settled.prompt
    );

    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let _report = notices.try_recv().unwrap();
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    for _ in 0..4 {
        sweep(&mut state).await;
    }
    assert!(
        !state.hub.is_running(member),
        "the removed work never came back"
    );
    assert!(notices.try_recv().is_err());
}

#[tokio::test]
async fn an_edit_settles_the_assignment_only_once_its_marker_is_gone() {
    let (fixture, _commands) = fixture();
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let (coordinator, member, id) =
        member_with_queued_assignment(&fixture, &mut state, &mut notices).await;
    let text = state.queued.views(member)[0].text.clone();

    let edit = |text: String| UiEvent::EditQueued {
        agent_id: member.to_hex(),
        id,
        text,
    };
    handle_ui_event(
        &mut state,
        edit(format!("{text}\n\nAlso the header.")),
        &links,
    )
    .await;
    assert_eq!(
        state.hub.inbox.owed_by(member),
        2,
        "its marker is still there"
    );
    assert!(notices.try_recv().is_err());

    handle_ui_event(&mut state, edit("Do the header instead.".into()), &links).await;
    assert_eq!(state.hub.inbox.owed_by(member), 1, "edited out");
    let settled = notices.try_recv().unwrap();
    assert_eq!(settled.to, coordinator);
    assert!(
        settled.prompt.contains("edited it out"),
        "{}",
        settled.prompt
    );
    fixture.gate.add_permits(1);
}

#[tokio::test]
async fn the_sweep_never_asks_for_work_its_coordinator_took_back() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let worker = code(&fixture, member);
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    assign_footer(&fixture, coordinator, &worker).await;
    let _lost = notices.try_recv().unwrap();
    let take_back = AssignArgs {
        to: Some(worker),
        agent: None,
        subject: String::new(),
        prompt: String::new(),
        todo_id: None,
        withdraw: true,
    };
    assign(&state.hub, coordinator, take_back).await.unwrap();
    assert!(state.hub.inbox.owing().is_empty(), "nothing owed any more");

    for _ in 0..4 {
        sweep(&mut state).await;
    }
    assert!(!state.hub.is_running(member), "not asked to carry it on");
    assert_ne!(state_of(&state.hub, member).state, "stalled");
    assert!(notices.try_recv().is_err());
}

#[tokio::test]
async fn a_member_failed_by_the_sweep_or_an_unstarted_start_leaves_the_software_factory() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);

    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let _lost = notices.try_recv().unwrap();
    state.hub.factory_flows.assign(member);
    state.hub.standing.retried(member);
    sweep(&mut state).await;
    sweep(&mut state).await;
    assert_eq!(state_of(&state.hub, member).state, "stalled");
    assert_eq!(
        state.hub.factory_flows.stage(member),
        None,
        "given up: out of the flow"
    );

    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let _lost = notices.try_recv();
    state.hub.factory_flows.assign(member);
    crate::event_loop::inbox::give_up_unstarted(&state, member, "its local model is not running");
    assert_eq!(state.hub.inbox.owed_by(member), 0, "given up");
    assert_eq!(
        state.hub.factory_flows.stage(member),
        None,
        "could not start: out of it too"
    );
}

#[tokio::test]
async fn the_sweep_leaves_a_turn_waiting_on_its_busy_machine_alone() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let _sent = notices.try_recv().unwrap();

    // Its cloud turn is under way, waiting for the one its machine runs.
    state.cloud.start_turn(member);
    for _ in 0..6 {
        sweep(&mut state).await;
    }
    assert_eq!(
        state.hub.inbox.owed_by(member),
        1,
        "still owed, not given up"
    );
    assert_ne!(state_of(&state.hub, member).state, "stalled");
    assert!(notices.try_recv().is_err(), "no retry, no report");
}

#[tokio::test]
async fn peers_see_each_queues_length_however_it_changes() {
    let (fixture, _commands) = fixture();
    let member = fixture.second;
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    let work = || delivery::prompt_work(SteerSource::User, "Next".into(), false);

    state.queued.push_back(member, work());
    state.queued.push_front(member, work());
    let view = state_of(&state.hub, member);
    assert_eq!(view.state, "queued");
    assert!(view.why.unwrap().starts_with('2'));
    state.queued.pop_front(member);
    assert!(state_of(&state.hub, member).why.unwrap().starts_with('1'));
    state.queued.drop_all(member);
    assert_eq!(
        state_of(&state.hub, member).state,
        "idle",
        "dropped as an archive does"
    );
    state.queued.push_back(member, work());
    state.queued.clear();
    assert_eq!(
        state_of(&state.hub, member).state,
        "idle",
        "cleared as a shutdown does"
    );
}
