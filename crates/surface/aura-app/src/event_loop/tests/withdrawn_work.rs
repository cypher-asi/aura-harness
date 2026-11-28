//! Work that must not start: an assignment its coordinator took back,
//! the queue of an archived member, and any work of a cloud channel's
//! member on this computer.

use super::inbox::{assign_footer, assign_footer_asking, channel_with_member, code, SLOW_FOOTER};
use super::*;
use crate::event_loop::inbox::{assign, AssignArgs};
use aura_exec_tools::todo::task_report_line;
use aura_store_db::NewChannel;

/// What the coordinator asks for to take back what `code` hasn't started.
fn take_back_from(code: &str) -> AssignArgs {
    AssignArgs {
        to: Some(code.to_string()),
        agent: None,
        subject: String::new(),
        prompt: String::new(),
        todo_id: None,
        withdraw: true,
    }
}

#[tokio::test]
async fn a_queued_assignment_taken_back_never_starts() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let worker = code(&fixture, member);
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    assign_footer_asking(&fixture, coordinator, &worker, SLOW_FOOTER).await;
    notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;
    assert!(state.hub.is_working(member));
    assign_footer(&fixture, coordinator, &worker).await;
    notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;
    assert_eq!(state.hub.inbox.owed_by(member), 2, "the second one waits");

    let said = assign(&state.hub, coordinator, take_back_from(&worker))
        .await
        .unwrap();
    assert!(said.contains("still reports"), "{said}");
    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    assert!(
        !state.hub.is_working(member),
        "what it was given next never ran"
    );
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    let report = notices.try_recv().unwrap();
    assert!(
        report.prompt.ends_with(&task_report_line("3", true)),
        "the running one still reports: {}",
        report.prompt
    );
    assert!(
        notices.try_recv().is_err(),
        "nothing for the one taken back"
    );
}

#[tokio::test]
async fn an_archived_members_queue_never_starts_and_what_it_owed_is_given_up() {
    let (fixture, _commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let worker = code(&fixture, member);
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    assign_footer_asking(&fixture, coordinator, &worker, SLOW_FOOTER).await;
    notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;
    assign_footer(&fixture, coordinator, &worker).await;
    notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;

    handle_ui_event(&mut state, UiEvent::ArchiveAgent(member.to_hex()), &links).await;
    let stopped = next_done(&mut done).await;
    handle_work_done(&mut state, stopped).await;

    assert!(!state.hub.is_working(member), "its queue was dropped");
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    let reports: Vec<String> = std::iter::from_fn(|| notices.try_recv().ok())
        .map(|notice| notice.prompt)
        .collect();
    assert_eq!(reports.len(), 2, "{reports:#?}");
    for report in &reports {
        assert!(report.ends_with(&task_report_line("3", false)), "{report}");
    }
    assert!(
        reports.iter().any(|r| r.contains("the agent was archived")),
        "{reports:#?}"
    );
}

/// A new channel `#site` whose agents run in the cloud, in a project at
/// `dir`. Returns its coordinator.
async fn cloud_channel(fixture: &Fixture, dir: &std::path::Path) -> AgentId {
    let (project_id, _) = super::support::open_project(&fixture.hub, dir);
    let in_cloud = NewChannel {
        slug: "site".to_string(),
        title: "Website relaunch".to_string(),
        placement: aura_store_db::ChannelPlacement::Cloud,
        project_id: Some(project_id),
        ..NewChannel::default()
    };
    channels::create_channel(&fixture.hub, in_cloud)
        .await
        .unwrap()
        .coordinator
}

#[tokio::test]
async fn a_cloud_channels_member_without_a_machine_never_works_here() {
    let (fixture, _commands) = fixture();
    let dir = tempfile::TempDir::new().unwrap();
    let coordinator = cloud_channel(&fixture, dir.path()).await;
    // A member whose start was lost, as after a restart: listed as a
    // cloud member, with no machine and no start waiting.
    let member = channels::add_agent(&fixture.hub, coordinator, "reviewer")
        .unwrap()
        .agent_id();
    subagents::lock(&fixture.hub.cloud_starts).clear();
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);

    // Work assigned to it tries to start its machine; without the cloud
    // (the fixture has no boxd key) it fails rather than running here.
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;
    assert!(
        !state.hub.is_working(member),
        "nothing runs on this computer"
    );
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    let report = notices.try_recv().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(
        report.prompt.contains("Could not start on a cloud machine"),
        "{}",
        report.prompt
    );
    assert!(state.hub.status.failure(member).is_some(), "shown in red");

    // A message typed into its chat doesn't run here either.
    handle_ui_event(&mut state, message(member, "hello there", true), &links).await;
    assert!(!state.hub.is_working(member));
}

#[tokio::test]
async fn cloud_members_are_known_again_after_a_restart() {
    let (fixture, _commands) = fixture();
    let dir = tempfile::TempDir::new().unwrap();
    let in_cloud = cloud_channel(&fixture, dir.path()).await;
    let cloud_member = channels::add_agent(&fixture.hub, in_cloud, "reviewer")
        .unwrap()
        .agent_id();
    let docs = NewChannel {
        slug: "docs".to_string(),
        ..NewChannel::default()
    };
    let here = channels::create_channel(&fixture.hub, docs)
        .await
        .unwrap()
        .coordinator;
    let local_member = channels::add_agent(&fixture.hub, here, "reviewer")
        .unwrap()
        .agent_id();

    let sets = channels::load_channels(
        fixture.hub.channels.as_ref(),
        &mut subagents::lock(&fixture.hub.registry),
    );
    assert!(sets.cloud.contains(&cloud_member));
    assert!(
        !sets.cloud.contains(&in_cloud),
        "its coordinator works here"
    );
    assert!(!sets.cloud.contains(&local_member));
}
