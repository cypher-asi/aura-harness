//! Members of a channel whose agents run in the cloud: they start on a
//! cloud machine of their own or fail, in red, telling whoever assigned
//! them work. They never fall back to working on this computer.

use super::inbox::{assign_footer, channel_with_member, code};
use super::*;
use crate::event_loop::inbox::{assign, AssignArgs};
use aura_exec_tools::todo::task_report_line;
use aura_store_db::NewChannel;

/// A new channel `#site` whose agents run in the cloud, in the project
/// at `dir` when there is one. Returns its coordinator.
async fn cloud_channel(fixture: &Fixture, dir: Option<&std::path::Path>) -> AgentId {
    let project_id = dir.map(|dir| super::support::open_project(&fixture.hub, dir).0);
    let in_cloud = NewChannel {
        slug: "site".to_string(),
        title: "Website relaunch".to_string(),
        placement: aura_store_db::ChannelPlacement::Cloud,
        project_id,
        ..NewChannel::default()
    };
    channels::create_channel(&fixture.hub, in_cloud)
        .await
        .unwrap()
        .coordinator
}

/// What the coordinator asks for: a new `reviewer` on the footer, for
/// its todo 5.
fn footer_for_new_reviewer() -> AssignArgs {
    AssignArgs {
        to: None,
        agent: Some("reviewer".to_string()),
        subject: "Footer".to_string(),
        prompt: "Make the footer links work.".to_string(),
        todo_id: Some("5".to_string()),
        withdraw: false,
    }
}

/// The errors `agent`'s chat was shown.
fn errors_shown(commands: &mut mpsc::Receiver<UiCommand>, agent: AgentId) -> Vec<String> {
    let hex = agent.to_hex();
    drain(commands)
        .into_iter()
        .filter(|(to, _)| to.as_deref() == Some(hex.as_str()))
        .filter_map(|(_, command)| match command {
            UiCommand::ShowError(text) => Some(text),
            _ => None,
        })
        .collect()
}

/// Assign the footer to a new member of the cloud channel `coordinator`
/// runs, and deliver it. Returns the member: its start failed, so no work
/// of it may be running.
async fn assign_to_new_member_whose_start_fails(
    state: &mut LoopState<'_>,
    notices: &mut tokio::sync::mpsc::UnboundedReceiver<notices::Notice>,
    coordinator: AgentId,
) -> AgentId {
    let said = assign(&state.hub, coordinator, footer_for_new_reviewer())
        .await
        .unwrap();
    assert!(said.contains("(new member)"), "{said}");
    let assignment = notices.try_recv().unwrap();
    let member = assignment.to;
    assert!(subagents::lock(&state.hub.cloud_starts).contains(&member));
    notices::handle_notice(state, assignment).await;
    assert!(!state.hub.is_working(member), "nothing runs here instead");
    assert!(!state.cloud.is_moving(member));
    member
}

/// The report `coordinator` got from `member`: given up on, saying why.
fn assert_failed_report(
    notices: &mut tokio::sync::mpsc::UnboundedReceiver<notices::Notice>,
    coordinator: AgentId,
    why: &str,
) {
    let report = notices.try_recv().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(
        report.prompt.contains(&format!(
            "Stopped before finishing: Could not start on a cloud machine: {why}"
        )),
        "{}",
        report.prompt
    );
    assert!(
        report.prompt.ends_with(&task_report_line("5", false)),
        "{}",
        report.prompt
    );
}

/// The member's row, as the agent lists and the channel's MEMBERS get it.
fn member_row(state: &LoopState<'_>, member: AgentId) -> aura_ui::events::AgentSummary {
    state.hub.member_rows().remove(&member).unwrap()
}

#[tokio::test]
async fn a_new_member_of_a_cloud_channel_without_a_project_fails_instead_of_working_here() {
    let (fixture, mut commands) = fixture();
    let coordinator = cloud_channel(&fixture, None).await;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);

    let member =
        assign_to_new_member_whose_start_fails(&mut state, &mut notices, coordinator).await;

    let why = "the channel has no project folder for a cloud machine to start from";
    assert_failed_report(&mut notices, coordinator, why);
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    assert!(errors_shown(&mut commands, member)
        .iter()
        .any(|error| error.contains(why) && error.contains("only in the cloud")));
    let failure = state.hub.status.failure(member).unwrap();
    assert!(failure.contains(why), "{failure}");
}

#[tokio::test]
async fn a_new_member_whose_cloud_cannot_be_reached_fails_instead_of_working_here() {
    let (fixture, mut commands) = fixture();
    let dir = tempfile::TempDir::new().unwrap();
    let coordinator = cloud_channel(&fixture, Some(dir.path())).await;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);

    // The fixture has no boxd key: the cloud can't be reached.
    let member =
        assign_to_new_member_whose_start_fails(&mut state, &mut notices, coordinator).await;

    let report = notices.try_recv().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(
        report
            .prompt
            .contains("Stopped before finishing: Could not start on a cloud machine: "),
        "{}",
        report.prompt
    );
    assert!(report.prompt.ends_with(&task_report_line("5", false)));
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    assert!(member_row(&state, member).error.is_some(), "shown in red");
    let _ = errors_shown(&mut commands, member);
}

#[tokio::test]
async fn a_member_whose_machine_did_not_start_fails_and_runs_nothing_here() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);

    // Its machine is starting: the assignment waits in its queue.
    state.cloud.begin_test_move(member);
    state.hub.status.set_starting(member, true);
    assert!(member_row(&state, member).starting, "told apart from idle");
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;
    assert!(!state.hub.is_working(member));

    // The start fails, or takes too long, past the recovery's retries.
    let why = "the machine did not start within 20 minutes";
    state.hub.recovery.exhaust_start_retries(member);
    state.cloud.fail_test_start(member, why);
    cloud::handle_ended(&mut state, cloud::Ended::Move(member)).await;

    assert!(!state.hub.is_working(member), "its work doesn't run here");
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    let report = notices.try_recv().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(
        report
            .prompt
            .contains(&format!("Could not start on a cloud machine: {why}")),
        "{}",
        report.prompt
    );
    assert!(report.prompt.ends_with(&task_report_line("3", false)));
    let row = member_row(&state, member);
    assert!(!row.starting);
    assert!(row.error.unwrap().contains(why));
}

#[tokio::test]
async fn a_member_whose_machine_started_takes_its_work_and_loses_its_error() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.hub.status.fail(member, "Its turn failed: earlier");

    state.cloud.begin_test_move(member);
    state.hub.status.set_starting(member, true);
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    let assignment = notices.try_recv().unwrap();
    notices::handle_notice(&mut state, assignment).await;
    // The move ended without failing: here, as the fixture has no cloud.
    cloud::handle_ended(&mut state, cloud::Ended::Move(member)).await;
    assert!(state.hub.is_working(member), "its work started");
    assert!(!member_row(&state, member).starting);
    assert_eq!(member_row(&state, member).error, None, "new work clears it");
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let report = notices.try_recv().unwrap();
    assert!(report.prompt.ends_with(&task_report_line("3", true)));
}
