//! An agent that begins to implement is out of plan mode, and no row it
//! is on shows `(P)` any more: asked to build while it plans, here or in
//! the cloud, or starting to edit in a cloud turn asked to plan.

use super::*;
use crate::cloud::RemoteEvent;
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::factory::flow::Stage;
use aura_store_db::NewChannel;
use aura_ui::events::AgentSummary;

/// A channel with the fixture's second agent as a member.
async fn channel_with_member(fixture: &Fixture) {
    let channel = channels::create_channel(
        &fixture.hub,
        NewChannel {
            slug: "site".to_string(),
            title: "Website relaunch".to_string(),
            ..NewChannel::default()
        },
    )
    .await
    .unwrap();
    let member = lock_registry_agent(fixture, fixture.second);
    channels::join_parent_channel(&fixture.hub, channel.coordinator, &member).unwrap();
}

async fn finish(state: &mut LoopState<'_>, done: &mut mpsc::Receiver<WorkDone>, agent: AgentId) {
    let finished = next_done(done).await;
    assert_eq!(finished.agent_id, agent);
    handle_work_done(state, finished).await;
}

/// Whether `agent`'s rows in the lists sent since last asked mark plan
/// mode: `None` when no list was sent with it.
fn rows_mark_plan(commands: &mut mpsc::Receiver<UiCommand>, agent: AgentId) -> Option<bool> {
    let hex = agent.to_hex();
    let rows = |rows: &[AgentSummary]| -> Option<bool> {
        rows.iter()
            .find(|row| row.id == hex)
            .map(|row| row.plan_mode)
    };
    drain(commands)
        .into_iter()
        .rev()
        .find_map(|(_, command)| match command {
            UiCommand::SetAgents(list) => rows(&list),
            UiCommand::SetChannels(channels) => channels.iter().find_map(|c| rows(&c.members)),
            _ => None,
        })
}

#[tokio::test]
async fn a_planning_member_told_to_implement_builds_out_of_plan_mode() {
    let (fixture, mut commands) = fixture();
    let member = fixture.second;
    channel_with_member(&fixture).await;
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(
        &mut state,
        message(member, "Collapse the members' subagents", true),
        &links,
    )
    .await;
    finish(&mut state, &mut done, member).await;
    assert_eq!(state.hub.factory_flows.stage(member), Some(Stage::Planning));
    assert_eq!(state.hub.mode_of(member), PermissionMode::Plan);
    assert_eq!(
        rows_mark_plan(&mut commands, member),
        Some(true),
        "(P) while it plans"
    );

    handle_ui_event(&mut state, message(member, "implement", true), &links).await;
    assert_eq!(
        state.hub.mode_of(member),
        PermissionMode::Agent,
        "out of plan mode"
    );
    assert_eq!(
        state.hub.factory_flows.stage(member),
        Some(Stage::Building),
        "its task builds, and is shipped and reported as a planned one"
    );
    assert_eq!(
        rows_mark_plan(&mut commands, member),
        Some(false),
        "no (P) on its rows"
    );
    finish(&mut state, &mut done, member).await;
    assert_ne!(state.hub.mode_of(member), PermissionMode::Plan);
    assert_ne!(state.hub.factory_flows.stage(member), Some(Stage::Planning));
}

#[tokio::test]
async fn switching_out_of_plan_mode_refreshes_every_row_at_once() {
    let (fixture, mut commands) = fixture();
    let member = fixture.first;
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let set = |mode| UiEvent::SetMode {
        agent_id: member.to_hex(),
        mode,
    };
    handle_ui_event(&mut state, set(PermissionMode::Plan), &links).await;
    assert_eq!(rows_mark_plan(&mut commands, member), Some(true));
    handle_ui_event(&mut state, set(PermissionMode::Agent), &links).await;
    assert_eq!(rows_mark_plan(&mut commands, member), Some(false));
}

#[tokio::test]
async fn a_cloud_turn_asked_to_plan_that_starts_editing_leaves_plan_mode() {
    let (fixture, mut commands) = fixture();
    let member = fixture.second;
    let hub = Arc::clone(&fixture.hub);
    let approval = hub.permissions_of(member).approval;
    hub.set_permissions(member, Permissions::new(PermissionMode::Plan, approval));
    hub.factory_flows.assign(member);
    hub.factory_flows.begin(member);
    assert_eq!(hub.factory_flows.stage(member), Some(Stage::Planning));

    let (tx, mut rx) = mpsc::channel(8);
    let ui = AgentUi::new(fixture.commands_tx.clone(), member);
    let marked = Arc::clone(&hub);
    let turn = tokio::spawn(async move {
        crate::event_loop::cloud::forward::forward_marking_waits(marked, member, &ui, &mut rx)
            .await;
    });
    let tool = |id: &str, name: &str| RemoteEvent::ToolStart {
        id: id.to_string(),
        name: name.to_string(),
    };
    // Reading and searching is planning.
    tx.send(tool("1", "read_file")).await.unwrap();
    tx.send(tool("2", "search_code")).await.unwrap();
    // Editing is implementation.
    tx.send(tool("3", "edit_file")).await.unwrap();
    drop(tx);
    turn.await.unwrap();

    assert_eq!(
        hub.mode_of(member),
        PermissionMode::Agent,
        "out of plan mode"
    );
    assert_eq!(hub.factory_flows.stage(member), Some(Stage::Building));
    assert_eq!(
        rows_mark_plan(&mut commands, member),
        Some(false),
        "no (P) on its rows"
    );
}

#[test]
fn editing_committing_and_pushing_implement_reading_does_not() {
    use crate::event_loop::modes::implements;
    for tool in [
        "write_file",
        "edit_file",
        "delete_file",
        "git_commit",
        "git_push",
        "git_ship",
    ] {
        assert!(implements(tool), "{tool}");
    }
    for tool in [
        "read_file",
        "search_code",
        "list_files",
        "create_plan",
        "todo_write",
    ] {
        assert!(!implements(tool), "{tool}");
    }
}

/// Handle every turn of `member` until it has nothing left to do.
async fn run_out(state: &mut LoopState<'_>, done: &mut mpsc::Receiver<WorkDone>, member: AgentId) {
    while state.hub.is_running(member) {
        let finished = next_done(done).await;
        handle_work_done(state, finished).await;
    }
}

#[tokio::test]
async fn a_new_task_after_implementing_plans_again_with_its_badge() {
    let (fixture, mut commands) = fixture();
    let member = fixture.second;
    channel_with_member(&fixture).await;
    // A repo it can ship to, so its task runs to the end.
    let origin = tempfile::tempdir().unwrap();
    super::factory_restart::ship_ready(&fixture.workspace, origin.path());
    let yolo = Permissions::new(PermissionMode::Agent, aura_ui::ApprovalLevel::Yolo);
    fixture.hub.set_permissions(member, yolo);
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let task = message(member, "Collapse the members' subagents", true);
    handle_ui_event(&mut state, task, &links).await;
    finish(&mut state, &mut done, member).await;
    handle_ui_event(&mut state, message(member, "implement", true), &links).await;
    finish(&mut state, &mut done, member).await;
    run_out(&mut state, &mut done, member).await;
    assert_ne!(state.hub.mode_of(member), PermissionMode::Plan);
    drain(&mut commands);

    let next = message(member, "Fold the archive by day", true);
    handle_ui_event(&mut state, next, &links).await;
    assert_eq!(state.hub.factory_flows.stage(member), Some(Stage::Planning));
    assert_eq!(
        state.hub.mode_of(member),
        PermissionMode::Plan,
        "plans first"
    );
    assert_eq!(
        rows_mark_plan(&mut commands, member),
        Some(true),
        "(P) is back"
    );
}

#[tokio::test]
async fn a_restart_mid_build_carries_on_building_out_of_plan_mode() {
    use crate::event_loop::inbox as owed;
    let (fixture, mut commands) = fixture();
    let member = fixture.second;
    channel_with_member(&fixture).await;
    // A repo it can ship to, so its task runs to the end.
    let origin = tempfile::tempdir().unwrap();
    super::factory_restart::ship_ready(&fixture.workspace, origin.path());
    let yolo = Permissions::new(PermissionMode::Agent, aura_ui::ApprovalLevel::Yolo);
    fixture.hub.set_permissions(member, yolo);
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    let task = message(member, "Collapse the members' subagents", true);
    handle_ui_event(&mut state, task, &links).await;
    finish(&mut state, &mut done, member).await;
    handle_ui_event(&mut state, message(member, "implement", true), &links).await;
    assert_eq!(state.hub.factory_flows.stage(member), Some(Stage::Building));
    assert!(state.hub.is_running(member), "its build runs");

    // The app dies mid-build and comes back with nothing in memory, the
    // member in the mode a new process starts it in.
    owed::save_now(&state).await;
    drop(next_done(&mut done).await);
    state.hub.stop_running(member);
    state.in_flight = 0;
    state.hub.inbox.forget_all();
    state.hub.factory_flows.forget_all();
    state.queued.clear();
    let yolo = Permissions::new(PermissionMode::Agent, aura_ui::ApprovalLevel::Yolo);
    state.hub.set_permissions(member, yolo);
    let restored = owed::restore_saved(&mut state).await;

    assert_eq!(state.hub.factory_flows.stage(member), Some(Stage::Building));
    assert_eq!(
        state.hub.mode_of(member),
        PermissionMode::Agent,
        "not back in plan mode"
    );
    assert_eq!(restored.queued_agents(), [member], "its build carries on");
    drain(&mut commands);
    owed::resume_saved(&mut state, restored).await;
    assert_ne!(state.hub.mode_of(member), PermissionMode::Plan);
    assert_ne!(rows_mark_plan(&mut commands, member), Some(true), "no (P)");
    run_out(&mut state, &mut done, member).await;
    assert_ne!(state.hub.factory_flows.stage(member), Some(Stage::Planning));
    assert_ne!(state.hub.mode_of(member), PermissionMode::Plan);
}
