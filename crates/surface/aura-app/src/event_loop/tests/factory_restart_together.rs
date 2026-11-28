//! A restart that cuts short a member mid-build and a member mid-turn
//! together: the software factory queues the first one's build again,
//! the resume step asks the second to carry on, and neither runs twice.

use super::factory_restart::{assigned, shipped_to_origin, through_the_plan};
use super::inbox::{code, SLOW_FOOTER};
use super::*;
use crate::event_loop::factory::flow::Stage;
use crate::event_loop::inbox::{self as owed, assign, AssignArgs};
use aura_ui::{ApprovalLevel, PermissionMode};

/// What a resumed turn's prompt starts with.
const RESUMED: &str = "The app restarted";

/// Join `agent` to the channel `coordinator` runs and give it the slow
/// footer, for todo 4: its turn stays under way until the gate opens.
async fn plain_member_mid_turn(
    fixture: &Fixture,
    state: &mut LoopState<'_>,
    notices: &mut tokio::sync::mpsc::UnboundedReceiver<notices::Notice>,
    (coordinator, agent): (AgentId, AgentId),
) {
    let joined = lock_registry_agent(fixture, agent);
    channels::join_parent_channel(&fixture.hub, coordinator, &joined).unwrap();
    let allowed = Permissions::new(PermissionMode::Agent, ApprovalLevel::Yolo);
    state.hub.set_permissions(agent, allowed);
    let args = AssignArgs {
        to: Some(code(fixture, agent).to_lowercase()),
        agent: None,
        subject: "Header".to_string(),
        prompt: SLOW_FOOTER.to_string(),
        todo_id: Some("4".to_string()),
        withdraw: false,
    };
    assign(&state.hub, coordinator, args).await.unwrap();
    notices::handle_notice(state, notices.try_recv().unwrap()).await;
    // Out of the factory's flow: its turn resumes on its own.
    state.hub.factory_flows.leave(agent);
    assert!(state.hub.is_running(agent));
}

/// The app dies with both `agents` mid-turn: the last save is the loop's,
/// their ends are never handled, and the new process holds nothing.
async fn crash(
    state: &mut LoopState<'_>,
    done: &mut mpsc::Receiver<WorkDone>,
    agents: [AgentId; 2],
) -> owed::Resume {
    owed::save_now(state).await;
    state.hub.cancel_all();
    for _ in agents {
        drop(next_done(done).await);
    }
    let fresh = Permissions::new(PermissionMode::Agent, ApprovalLevel::Yolo);
    for agent in agents {
        state.hub.stop_running(agent);
        state.hub.set_permissions(agent, fresh);
    }
    state.in_flight = 0;
    state.hub.inbox.forget_all();
    state.hub.factory_flows.forget_all();
    state.queued.clear();
    owed::restore_saved(state).await
}

/// Handle every turn until none runs; the agent of each, in order.
async fn run_all(state: &mut LoopState<'_>, done: &mut mpsc::Receiver<WorkDone>) -> Vec<AgentId> {
    let mut ran = Vec::new();
    while state.in_flight > 0 {
        let finished = next_done(done).await;
        ran.push(finished.agent_id);
        handle_work_done(state, finished).await;
    }
    ran
}

/// How many of `agent`'s prompts say `text`.
fn prompts_saying(state: &LoopState<'_>, agent: AgentId, text: &str) -> usize {
    let asked = user_texts(state, agent);
    asked.iter().filter(|t| t.contains(text)).count()
}

#[tokio::test]
async fn a_member_mid_build_and_a_member_mid_turn_each_carry_on_once_after_a_restart() {
    let (fixture, _commands) = fixture();
    let origin = tempfile::tempdir().unwrap();
    let (builder, plain) = (fixture.second, fixture.first);
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let coordinator = assigned(&fixture, &mut state, &mut notices, origin.path()).await;
    through_the_plan(&mut state, &mut done, builder).await;
    plain_member_mid_turn(&fixture, &mut state, &mut notices, (coordinator, plain)).await;

    let restored = crash(&mut state, &mut done, [builder, plain]).await;
    assert_eq!(
        restored.queued_agents(),
        [builder],
        "only the build is queued"
    );
    assert!(state.hub.factory_flows.resumes_after_restart(builder));
    assert_eq!(
        state.hub.factory_flows.stage(builder),
        Some(Stage::Building)
    );

    // The resumed turn carries on the slow footer, so the mock holds it.
    fixture.gate.add_permits(4);
    owed::resume_saved(&mut state, restored).await;
    assert!(state.hub.is_running(builder), "its build runs");
    assert!(state.hub.is_running(plain), "it carries on");
    let ran = run_all(&mut state, &mut done).await;

    let turns = |agent| ran.iter().filter(|ran| **ran == agent).count();
    assert_eq!(turns(plain), 1, "the plain member ran once: {ran:?}");
    assert_eq!(turns(builder), 2, "the build once, then the ship: {ran:?}");
    assert_eq!(prompts_saying(&state, plain, RESUMED), 1);
    assert_eq!(
        prompts_saying(&state, builder, RESUMED),
        0,
        "the factory's build is its resume, not a second turn"
    );
    assert!(shipped_to_origin(origin.path()), "it built and shipped");
    assert_eq!(state.hub.inbox.owed_by(builder), 0);
    assert_eq!(state.hub.inbox.owed_by(plain), 0);
    let reports: Vec<notices::Notice> = std::iter::from_fn(|| notices.try_recv().ok()).collect();
    for todo in ["3", "4"] {
        let line = aura_exec_tools::todo::task_report_line(todo, true);
        assert!(
            reports
                .iter()
                .any(|r| r.to == coordinator && r.prompt.ends_with(&line)),
            "no finished report for todo {todo} in {:?}",
            reports.iter().map(|r| &r.prompt).collect::<Vec<_>>()
        );
    }
}
