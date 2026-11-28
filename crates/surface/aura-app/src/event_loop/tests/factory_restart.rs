//! Restarts and the software factory: a member cut off mid-plan,
//! mid-build or mid-ship carries on at its stage after the app comes back,
//! ships, and its held-back report reaches the coordinator.

use super::inbox::{assign_footer_asking, channel_with_member, code};
use super::*;
use crate::event_loop::factory::flow::Stage;
use crate::event_loop::inbox as owed;
use crate::event_loop::subagents::lock;
use aura_exec_tools::todo::task_report_line;
use aura_ui::{ApprovalLevel, PermissionMode};
use std::path::Path;
use tokio::sync::mpsc::UnboundedReceiver;

/// What the coordinator asks: the scripted model plans it with
/// `create_plan` ([`super::provider`]).
const TASK: &str = "factory: plan the footer fix";

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Make `workspace` a repo on `aura/footer` with an origin that has
/// `main`, so `git_ship` can commit, pull main and push.
fn ship_ready(workspace: &Path, origin: &Path) {
    git(origin, &["init", "-q", "--bare", "-b", "main"]);
    git(workspace, &["init", "-q", "-b", "main"]);
    for (key, value) in [("user.name", "t"), ("user.email", "t@t")] {
        git(workspace, &["config", key, value]);
    }
    std::fs::write(workspace.join("README"), "footer\n").unwrap();
    git(workspace, &["add", "-A"]);
    git(workspace, &["commit", "-qm", "first"]);
    let url = origin.to_string_lossy().into_owned();
    git(workspace, &["remote", "add", "origin", &url]);
    git(workspace, &["push", "-q", "origin", "main"]);
    git(workspace, &["checkout", "-q", "-b", "aura/footer"]);
}

/// The app dies while `member`'s turn runs: the last save is what the
/// loop wrote after that event. The turn's end is never heard, its
/// session is reopened from the record, and the next process starts
/// with nothing in memory. Returns the agents the restore gave work.
async fn crash_mid_turn(
    state: &mut LoopState<'_>,
    done: &mut mpsc::Receiver<WorkDone>,
    member: AgentId,
) -> Vec<AgentId> {
    owed::keep_saved(state).unwrap().await.unwrap();
    drop(next_done(done).await);
    state.hub.stop_running(member);
    state.in_flight = 0;
    state.hub.inbox.forget_all();
    state.hub.factory_flows.forget_all();
    state.queued.clear();
    // A new process starts every agent in its default mode.
    let fresh = Permissions::new(PermissionMode::Agent, ApprovalLevel::Yolo);
    state.hub.set_permissions(member, fresh);
    owed::restore_saved(state).await
}

/// Handle every turn of `member` until it has nothing left to do.
async fn run_out(state: &mut LoopState<'_>, done: &mut mpsc::Receiver<WorkDone>, member: AgentId) {
    while state.hub.is_running(member) {
        let finished = next_done(done).await;
        handle_work_done(state, finished).await;
    }
}

/// The member's report to `coordinator`, closing todo 3, done.
fn assert_report_delivered(notices: &mut UnboundedReceiver<notices::Notice>, to: AgentId) {
    let reports: Vec<notices::Notice> = std::iter::from_fn(|| notices.try_recv().ok()).collect();
    let line = task_report_line("3", true);
    assert!(
        reports
            .iter()
            .any(|report| report.to == to && report.prompt.ends_with(&line)),
        "no finished report to the coordinator in {:?}",
        reports.iter().map(|r| &r.prompt).collect::<Vec<_>>()
    );
}

/// A member of a local channel, in a repo it can ship from, given the
/// footer task; its planning turn is under way. Returns the coordinator.
async fn assigned(
    fixture: &Fixture,
    state: &mut LoopState<'_>,
    notices: &mut UnboundedReceiver<notices::Notice>,
    origin: &Path,
) -> AgentId {
    let coordinator = channel_with_member(fixture).await;
    let member = fixture.second;
    ship_ready(&fixture.workspace, origin);
    let allowed = Permissions::new(PermissionMode::Agent, ApprovalLevel::Yolo);
    state.hub.set_permissions(member, allowed);
    owed::restore_saved(state).await;
    assign_footer_asking(fixture, coordinator, &code(fixture, member), TASK).await;
    let assignment = notices.try_recv().unwrap();
    notices::handle_notice(state, assignment).await;
    assert_eq!(state.hub.factory_flows.stage(member), Some(Stage::Planning));
    assert!(state.hub.is_running(member), "its planning turn runs");
    coordinator
}

/// The branch `member` shipped, as origin has it.
fn shipped_to_origin(origin: &Path) -> bool {
    std::process::Command::new("git")
        .args(["rev-parse", "--verify", "-q", "refs/heads/aura/footer"])
        .current_dir(origin)
        .output()
        .is_ok_and(|out| out.status.success())
}

#[tokio::test]
async fn a_member_cut_off_mid_plan_plans_again_in_plan_mode_then_builds_and_ships() {
    let (fixture, _commands) = fixture();
    let origin = tempfile::tempdir().unwrap();
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let coordinator = assigned(&fixture, &mut state, &mut notices, origin.path()).await;

    let restored = crash_mid_turn(&mut state, &mut done, member).await;
    assert_eq!(restored, [member], "its planning turn is queued again");
    assert_eq!(state.hub.factory_flows.stage(member), Some(Stage::Planning));
    assert_eq!(
        state.hub.mode_of(member),
        PermissionMode::Plan,
        "still planning"
    );
    assert!(state.hub.factory_flows.resumes_after_restart(member));
    assert_eq!(
        state.hub.inbox.owed_by(member),
        1,
        "the report is still owed"
    );

    owed::resume_saved(&mut state, restored).await;
    assert!(
        !state.hub.factory_flows.resumes_after_restart(member),
        "it started"
    );
    run_out(&mut state, &mut done, member).await;

    assert!(shipped_to_origin(origin.path()), "it built and shipped");
    assert_eq!(
        state.hub.factory_flows.stage(member),
        None,
        "the flow ended"
    );
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    assert_report_delivered(&mut notices, coordinator);
}

/// Let `member`'s planning turn end: the plan is written and its build
/// starts.
async fn through_the_plan(
    state: &mut LoopState<'_>,
    done: &mut mpsc::Receiver<WorkDone>,
    member: AgentId,
) {
    let planned = next_done(done).await;
    handle_work_done(state, planned).await;
    assert_eq!(state.hub.factory_flows.stage(member), Some(Stage::Building));
    assert!(state.hub.is_running(member), "its build runs");
}

#[tokio::test]
async fn a_member_cut_off_mid_build_builds_its_plan_again_and_ships() {
    let (fixture, _commands) = fixture();
    let origin = tempfile::tempdir().unwrap();
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let coordinator = assigned(&fixture, &mut state, &mut notices, origin.path()).await;
    through_the_plan(&mut state, &mut done, member).await;
    let plan = state.hub.factory_flows.plan_of(member).unwrap();

    let restored = crash_mid_turn(&mut state, &mut done, member).await;
    assert_eq!(restored, [member], "its build is queued again");
    assert_eq!(state.hub.factory_flows.stage(member), Some(Stage::Building));
    assert_eq!(state.hub.factory_flows.plan_of(member), Some(plan));
    assert_eq!(
        state.hub.inbox.owed_by(member),
        1,
        "the build holds the report back"
    );

    owed::resume_saved(&mut state, restored).await;
    run_out(&mut state, &mut done, member).await;
    assert!(shipped_to_origin(origin.path()));
    assert_eq!(state.hub.factory_flows.stage(member), None);
    assert_report_delivered(&mut notices, coordinator);
}

#[tokio::test]
async fn a_member_cut_off_mid_ship_is_asked_to_ship_again() {
    let (fixture, _commands) = fixture();
    let origin = tempfile::tempdir().unwrap();
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let coordinator = assigned(&fixture, &mut state, &mut notices, origin.path()).await;
    through_the_plan(&mut state, &mut done, member).await;
    let built = next_done(&mut done).await;
    handle_work_done(&mut state, built).await;
    assert_eq!(state.hub.factory_flows.stage(member), Some(Stage::Shipping));

    let restored = crash_mid_turn(&mut state, &mut done, member).await;
    assert_eq!(restored, [member], "the ship prompt is queued again");
    assert_eq!(state.hub.factory_flows.stage(member), Some(Stage::Shipping));
    owed::resume_saved(&mut state, restored).await;
    run_out(&mut state, &mut done, member).await;
    assert!(shipped_to_origin(origin.path()));
    assert_eq!(state.hub.factory_flows.stage(member), None);
    assert_report_delivered(&mut notices, coordinator);
}

#[tokio::test]
async fn members_out_of_the_flow_stay_out_after_a_restart() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    assign_footer_asking(&fixture, coordinator, &code(&fixture, member), TASK).await;
    let _lost = notices.try_recv().unwrap();
    // It failed: the stall sweep or a start that never ran took it out.
    state.hub.factory_flows.assign(member);
    state.hub.factory_flows.leave(member);
    owed::keep_saved(&state).unwrap().await.unwrap();
    state.hub.factory_flows.forget_all();
    assert!(owed::restore_saved(&mut state).await.is_empty());
    assert_eq!(state.hub.factory_flows.stage(member), None);

    // An archived member's stage is dropped too.
    state.hub.factory_flows.assign(member);
    owed::keep_saved(&state).unwrap().await.unwrap();
    lock(&state.hub.registry).archive(member).unwrap();
    state.hub.factory_flows.forget_all();
    owed::restore_saved(&mut state).await;
    assert_eq!(state.hub.factory_flows.stage(member), None);
}

#[tokio::test]
async fn a_member_waiting_on_a_reply_keeps_planning_without_new_work() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    owed::restore_saved(&mut state).await;
    assign_footer_asking(&fixture, coordinator, &code(&fixture, member), TASK).await;
    let _lost = notices.try_recv().unwrap();
    // It planned, asked a question, and waits for the answer.
    state.hub.factory_flows.assign(member);
    assert!(state.hub.factory_flows.begin(member));
    owed::keep_saved(&state).unwrap().await.unwrap();
    state.hub.factory_flows.forget_all();
    assert!(
        owed::restore_saved(&mut state).await.is_empty(),
        "nothing to run"
    );
    assert_eq!(state.hub.factory_flows.stage(member), Some(Stage::Planning));
    assert_eq!(state.hub.mode_of(member), PermissionMode::Plan);
}
