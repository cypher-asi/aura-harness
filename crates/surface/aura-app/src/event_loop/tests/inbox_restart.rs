//! Restarts and the inbox: what members owe, the inbox messages queued
//! for them and those not yet delivered come back, and the next turn
//! that ends well reports to whoever assigned the work.

use super::inbox::{assign_footer, channel_with_member, code};
use super::*;
use crate::event_loop::inbox::{self as owed, assign, AssignArgs};
use crate::event_loop::stalls::sweep;
use crate::event_loop::subagents::lock;
use crate::event_loop::turns::{Prompt, Request, Work};
use aura_exec_tools::todo::task_report_line;
use tokio::sync::mpsc::UnboundedReceiver;

/// Save the inbox as the loop does when it quits, forget it all as a
/// new process would (the notices still on their way too), and restore
/// it as the loop does when it starts.
pub(super) async fn restart(
    state: &mut LoopState<'_>,
    notices: &mut UnboundedReceiver<notices::Notice>,
) -> owed::Resume {
    owed::flush_saved(state).await;
    while notices.try_recv().is_ok() {}
    state.hub.inbox.forget_all();
    state.queued.clear();
    owed::restore_saved(state).await
}

/// Queue `assignment` for `member` as delivery does for a busy one.
fn queue(state: &mut LoopState<'_>, member: AgentId, assignment: &notices::Notice) {
    owed::delivered(&state.hub, member, &assignment.prompt);
    let work = Work::Prompt {
        prompt: Prompt::inbox(assignment.prompt.clone(), Request::New),
        shown: false,
    };
    state.queued.push_back(member, work);
}

/// The saved inbox file of `state`'s hub, as text.
fn saved_text(state: &LoopState<'_>) -> String {
    let path = lock(&state.hub.registry).data_dir().join(owed::SAVED_FILE);
    std::fs::read_to_string(path).unwrap()
}

/// The report `notices` holds next: to `coordinator`, closing todo 3.
/// The loop gave it to the coordinator, as delivery does.
fn assert_reported(
    hub: &AgentHub,
    notices: &mut UnboundedReceiver<notices::Notice>,
    to: AgentId,
    ok: bool,
) {
    let report = notices.try_recv().unwrap();
    owed::delivered(hub, report.to, &report.prompt);
    assert_eq!(report.to, to);
    assert!(
        report.prompt.ends_with(&task_report_line("3", ok)),
        "{}",
        report.prompt
    );
}

#[tokio::test]
async fn a_debt_saved_before_a_crash_is_swept_and_reported_by_the_next_turn() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    assert!(
        owed::restore_saved(&mut state)
            .await
            .queued_agents()
            .is_empty(),
        "no file yet"
    );
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    assert!(
        saved_text(&state).contains("\"todo\":\"3\""),
        "assign saved it before it returned"
    );
    // Given to it, then lost with its turn.
    let lost = notices.try_recv().unwrap();
    owed::delivered(&state.hub, member, &lost.prompt);
    // Saved as the loop saves after each event; then the app dies.
    owed::save_now(&state).await;
    assert!(owed::keep_saved(&state).is_none(), "unchanged: not written");
    state.hub.inbox.forget_all();
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    owed::restore_saved(&mut state).await;
    assert_eq!(state.hub.inbox.owed_by(member), 1);

    sweep(&mut state).await;
    sweep(&mut state).await;
    assert!(state.hub.is_running(member), "the sweep sees the debt");
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    assert_reported(&state.hub, &mut notices, coordinator, true);
}

#[tokio::test]
async fn a_queued_assignment_runs_after_a_restart_and_reports_to_its_assigner() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let assignment = notices.try_recv().unwrap();
    queue(&mut state, member, &assignment);

    let restored = restart(&mut state, &mut notices).await;
    assert_eq!(restored.queued_agents(), [member]);
    assert_eq!(state.queued.len(member), 1);
    assert_eq!(state.hub.inbox.owed_by(member), 1);
    owed::resume_saved(&mut state, restored).await;
    assert!(state.hub.is_running(member));
    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, member);
    handle_work_done(&mut state, finished).await;

    let asked = user_texts(&state, member);
    assert!(
        asked.iter().any(|text| text == &assignment.prompt),
        "{asked:?}"
    );
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    assert_reported(&state.hub, &mut notices, coordinator, true);
}

#[tokio::test]
async fn an_undelivered_assignment_is_delivered_after_a_restart() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    // Sent, still in the notice channel as the app quits.
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;

    assert!(restart(&mut state, &mut notices)
        .await
        .queued_agents()
        .is_empty());
    let again = notices.try_recv().unwrap();
    assert_eq!(again.to, member);
    assert!(again.prompt.contains("Make the footer links work."));
    assert_eq!(state.hub.inbox.owed_by(member), 1);

    notices::handle_notice(&mut state, again).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    assert_reported(&state.hub, &mut notices, coordinator, true);
}

#[tokio::test]
async fn work_taken_back_stays_taken_back_across_a_restart() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let worker = code(&fixture, member);
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    assign_footer(&fixture, coordinator, &worker).await;
    let assignment = notices.try_recv().unwrap();
    queue(&mut state, member, &assignment);
    let withdraw = AssignArgs {
        to: Some(worker.to_lowercase()),
        agent: None,
        subject: String::new(),
        prompt: String::new(),
        todo_id: None,
        withdraw: true,
    };
    assign(&state.hub, coordinator, withdraw).await.unwrap();

    let restored = restart(&mut state, &mut notices).await;
    assert!(saved_text(&state).contains("\"withdrawn\":true"));
    assert_eq!(state.hub.inbox.owed_by(member), 1, "owed until reached");
    owed::resume_saved(&mut state, restored).await;
    assert!(!state.hub.is_running(member), "dropped unrun");
    assert_eq!(state.queued.len(member), 0);
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    assert!(notices.try_recv().is_err(), "nobody is told");
}

#[tokio::test]
async fn work_given_up_stays_given_up_across_a_restart() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let assignment = notices.try_recv().unwrap();
    owed::delivered(&state.hub, member, &assignment.prompt);
    owed::give_up_all(&state.hub, member, "its local model is not running");
    assert_reported(&state.hub, &mut notices, coordinator, false);

    assert!(restart(&mut state, &mut notices)
        .await
        .queued_agents()
        .is_empty());
    let saved = saved_text(&state);
    assert!(saved.contains("\"owed\":[]"), "not owed: {saved}");
    assert!(!saved.contains("Make the footer"), "nor queued: {saved}");
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    assert!(notices.try_recv().is_err(), "not told again");
}

/// The id of the one item queued for `agent`.
fn only_queued_id(state: &LoopState<'_>, agent: AgentId) -> u64 {
    let [item] = state.queued.views(agent).try_into().unwrap();
    item.id
}

#[tokio::test]
async fn a_queued_assignment_the_user_removed_stays_gone_across_a_restart() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let assignment = notices.try_recv().unwrap();
    queue(&mut state, member, &assignment);
    owed::keep_saved(&state).unwrap().await.unwrap();
    assert!(saved_text(&state).contains("Make the footer links work."));

    // The user takes it out of the queue: its debt is settled, and the
    // save after that event leaves both out.
    let remove = UiEvent::RemoveQueued {
        agent_id: member.to_hex(),
        id: only_queued_id(&state, member),
    };
    handle_ui_event(&mut state, remove, &links).await;
    assert_reported(&state.hub, &mut notices, coordinator, false);
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    owed::keep_saved(&state).unwrap().await.unwrap();
    assert!(
        !saved_text(&state).contains("Make the footer"),
        "{}",
        saved_text(&state)
    );

    let restored = restart(&mut state, &mut notices).await;
    assert!(restored.queued_agents().is_empty());
    assert_eq!(state.queued.len(member), 0, "the item stays gone");
    assert_eq!(state.hub.inbox.owed_by(member), 0, "the debt stays settled");
    assert!(state.hub.inbox.owing().is_empty(), "the sweep sees nothing");
    assert!(notices.try_recv().is_err(), "not told again");
}

#[tokio::test]
async fn a_queued_assignment_edited_with_its_marker_kept_survives_a_restart() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let assignment = notices.try_recv().unwrap();
    queue(&mut state, member, &assignment);

    // The user adds to it; its marker stays, so it is still owed.
    let edited = format!("{}\n\nAlso fix the header.", assignment.prompt);
    let edit = UiEvent::EditQueued {
        agent_id: member.to_hex(),
        id: only_queued_id(&state, member),
        text: edited.clone(),
    };
    handle_ui_event(&mut state, edit, &links).await;
    assert!(notices.try_recv().is_err(), "nobody is told");
    let before = state.hub.inbox.owing();

    let restored = restart(&mut state, &mut notices).await;
    assert_eq!(restored.queued_agents(), [member]);
    assert_eq!(
        state.hub.inbox.owing(),
        before,
        "owed as before the restart"
    );
    let [item] = state.queued.views(member).try_into().unwrap();
    assert_eq!(item.text, edited, "the edit is kept");

    owed::resume_saved(&mut state, restored).await;
    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, member);
    handle_work_done(&mut state, finished).await;
    assert!(user_texts(&state, member).contains(&edited));
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    assert_reported(&state.hub, &mut notices, coordinator, true);
}
