//! Changing a channel's project through the UI (`UiEvent::UpdateChannel`)
//! with members spawned in it: what moves, what the UI is sent, and what
//! a busy coordinator refuses.

use super::support::open_project;
use super::*;
use crate::event_loop::subagents::tests::request;
use aura_ui::events::{AgentSummary, ChannelSummary, ProjectSummary};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Channel `#site` in project A, made and opened through the UI, with a
/// reviewer the coordinator spawned; project B is open beside it.
struct Site {
    channel_id: String,
    coordinator: AgentId,
    member: AgentId,
    a: String,
    a_folder: PathBuf,
    b: String,
    b_folder: PathBuf,
    _dirs: [TempDir; 2],
}

async fn site_in_a(
    fixture: &Fixture,
    state: &mut LoopState<'_>,
    commands: &mut mpsc::Receiver<UiCommand>,
    links: &mpsc::Sender<auth::LinkOutcome>,
) -> Site {
    let (dir_a, dir_b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let (a, a_folder) = open_project(&fixture.hub, dir_a.path());
    let (b, b_folder) = open_project(&fixture.hub, dir_b.path());
    let create = UiEvent::CreateChannel {
        slug: "site".to_string(),
        title: "Website relaunch".to_string(),
        description: String::new(),
        project_id: Some(a.clone()),
    };
    handle_ui_event(state, create, links).await;
    let channel = (fixture.hub.channels.as_ref().unwrap().by_slug("site"))
        .unwrap()
        .unwrap();
    assert_eq!(state.active, channel.coordinator);
    let member = fixture
        .dispatcher
        .dispatch(request(channel.coordinator, "reviewer", "Review the diff"))
        .await
        .unwrap()
        .child_agent_id
        .unwrap();
    let _ = drain(commands);
    Site {
        channel_id: channel.id,
        coordinator: channel.coordinator,
        member,
        a,
        a_folder,
        b,
        b_folder,
        _dirs: [dir_a, dir_b],
    }
}

pub(super) fn update(
    channel_id: &str,
    title: &str,
    description: &str,
    project_id: Option<&str>,
) -> UiEvent {
    UiEvent::UpdateChannel {
        channel_id: channel_id.to_string(),
        title: title.to_string(),
        description: description.to_string(),
        project_id: project_id.map(str::to_string),
    }
}

pub(super) fn record(fixture: &Fixture, channel_id: &str) -> aura_store_db::ChannelRecord {
    (fixture.hub.channels.as_ref().unwrap().get(channel_id))
        .unwrap()
        .unwrap()
}

/// The project and own folder of `agent`, in the registry and in its
/// stored row.
type Placements = [(Option<String>, Option<PathBuf>); 2];

pub(super) fn placements(fixture: &Fixture, agent: AgentId) -> Placements {
    let registry = subagents::lock(&fixture.hub.registry);
    let found = registry.find(agent).unwrap();
    let rows = registry.store().load().unwrap().agents;
    let row = rows.iter().find(|row| row.agent_id == agent).unwrap();
    [
        (found.project_id.clone(), found.folder.clone()),
        (row.project_id.clone(), row.folder.clone()),
    ]
}

/// The last projects, Agents panel rows and channels sent.
type Lists = (
    Option<Vec<ProjectSummary>>,
    Option<Vec<AgentSummary>>,
    Option<Vec<ChannelSummary>>,
);

/// The last projects, Agents panel rows and channels sent, if any.
fn last_lists(sent: &[(Option<String>, UiCommand)]) -> Lists {
    let (mut projects, mut rows, mut channels) = (None, None, None);
    for (_, command) in sent {
        match command {
            UiCommand::SetProjects(list) => projects = Some(list.clone()),
            UiCommand::SetAgents(list) => rows = Some(list.clone()),
            UiCommand::SetChannels(list) => channels = Some(list.clone()),
            _ => {}
        }
    }
    (projects, rows, channels)
}

/// How many listed agents `projects` counts under `project_id`.
fn listed(projects: &[ProjectSummary], project_id: &str) -> Option<usize> {
    projects
        .iter()
        .find(|p| p.id == project_id)
        .map(|p| p.listed_agents)
}

/// The project the row of `agent` in `rows` is listed under; `None` when
/// it is not listed.
fn listed_under(rows: &[AgentSummary], agent: AgentId) -> Option<Option<String>> {
    rows.iter()
        .find(|row| row.id == agent.to_hex())
        .map(|row| row.project_id.clone())
}

pub(super) fn errors(sent: &[(Option<String>, UiCommand)]) -> Vec<String> {
    sent.iter()
        .filter_map(|(_, c)| match c {
            UiCommand::ShowError(problem) => Some(problem.clone()),
            _ => None,
        })
        .collect()
}

/// Whether `sent` shows the tasks of channel `channel_id`.
pub(super) fn sent_channel_tasks(sent: &[(Option<String>, UiCommand)], channel_id: &str) -> bool {
    sent.iter().any(|(_, c)| {
        matches!(
            c,
            UiCommand::SetChannelTasks { channel_id: id, .. } if id == channel_id
        )
    })
}

/// Whether `sent` points the Sandbox panel anywhere.
pub(super) fn sent_sandbox_root(sent: &[(Option<String>, UiCommand)]) -> bool {
    sent.iter()
        .any(|(_, c)| matches!(c, UiCommand::SetSandboxRoot(_)))
}

/// Whether `sent` points the Sandbox panel at `root`.
pub(super) fn sent_sandbox_root_at(sent: &[(Option<String>, UiCommand)], root: &Path) -> bool {
    sent.iter()
        .any(|(_, c)| matches!(c, UiCommand::SetSandboxRoot(r) if r.as_path() == root))
}

/// The folder channel `slug` works in when it has no project.
fn channel_folder(fixture: &Fixture, slug: &str) -> PathBuf {
    (subagents::lock(&fixture.hub.registry).data_dir())
        .join("channels")
        .join(slug)
}

/// After `#site` moved from A to B: the coordinator reopened in B, and
/// the UI was sent B's folder, the channel's tasks and lists under B.
fn assert_moved_to_b(
    fixture: &Fixture,
    state: &LoopState<'_>,
    site: &Site,
    before: &Arc<Kernel>,
    sent: &[(Option<String>, UiCommand)],
) {
    assert_eq!(errors(sent), Vec::<String>::new());
    assert_eq!(
        record(fixture, &site.channel_id).project_id.as_deref(),
        Some(site.b.as_str())
    );
    let session = &state.idle[&site.coordinator];
    assert!(!Arc::ptr_eq(&session.kernel, before), "a new session");
    assert_eq!(session.kernel.agent_workspace(), site.b_folder);
    assert!(sent_sandbox_root_at(sent, &site.b_folder), "{sent:?}");
    // The open channel's tasks are read again from where it works now.
    assert!(sent_channel_tasks(sent, &site.channel_id), "{sent:?}");
    let (projects, rows, channels) = last_lists(sent);
    let (projects, rows) = (
        projects.expect("the projects are sent"),
        rows.expect("the agent list is sent"),
    );
    for agent in [site.coordinator, site.member] {
        assert_eq!(
            listed_under(&rows, agent),
            Some(Some(site.b.clone())),
            "{} is listed under B",
            agent.to_hex()
        );
    }
    assert_eq!(listed(&projects, &site.a), Some(1), "A keeps only its own");
    assert_eq!(listed(&projects, &site.b), Some(3));
    let channels = channels.expect("the channels are sent");
    let summary = channels.iter().find(|c| c.id == site.channel_id).unwrap();
    let project = summary.project.as_ref().expect("the summary has a project");
    assert_eq!(project.id, site.b);
    assert_eq!(project.folder, site.b_folder.display().to_string());
}

/// After `#site`'s project was cleared: the members left the Projects
/// tree and the coordinator works in the channel's folder.
fn assert_cleared(
    fixture: &Fixture,
    state: &LoopState<'_>,
    site: &Site,
    sent: &[(Option<String>, UiCommand)],
) {
    let (coordinator, member) = (site.coordinator, site.member);
    assert_eq!(errors(sent), Vec::<String>::new());
    assert_eq!(record(fixture, &site.channel_id).project_id, None);
    let channel_folder = channel_folder(fixture, "site");
    assert_eq!(
        state.idle[&coordinator].kernel.agent_workspace(),
        channel_folder
    );
    assert!(sent_sandbox_root_at(sent, &channel_folder), "{sent:?}");
    assert!(sent_channel_tasks(sent, &site.channel_id), "{sent:?}");
    let (_, rows, channels) = last_lists(sent);
    let rows = rows.expect("the agent list is sent");
    for agent in [coordinator, member] {
        assert_eq!(listed_under(&rows, agent), None, "{}", agent.to_hex());
    }
    assert_eq!(
        *subagents::lock(&fixture.hub.hidden_members),
        [coordinator, member].into()
    );
    let channels = channels.expect("the channels are sent");
    let summary = channels.iter().find(|c| c.id == site.channel_id).unwrap();
    assert_eq!(summary.project, None);
}

#[tokio::test]
async fn switching_a_channels_project_in_the_ui_moves_its_members_and_reopens_the_coordinator_there(
) {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let site = site_in_a(&fixture, &mut state, &mut commands, &links).await;
    let before = Arc::clone(&state.idle[&site.coordinator].kernel);
    assert_eq!(before.agent_workspace(), site.a_folder);
    // Each project has the agent it was opened with; A has the channel's too.
    // These counts are the UI-level view of the registry checks in `channels/tests.rs`.
    let [UiCommand::SetProjects(projects), ..] = fixture.hub.agent_lists() else {
        panic!("the lists come as projects, agents, archive");
    };
    assert_eq!(listed(&projects, &site.a), Some(3));
    assert_eq!(listed(&projects, &site.b), Some(1));

    // A to B.
    let to_b = update(&site.channel_id, "", "", Some(&site.b));
    handle_ui_event(&mut state, to_b, &links).await;
    let sent = drain(&mut commands);
    assert_moved_to_b(&fixture, &state, &site, &before, &sent);

    // B to none.
    let cleared = update(&site.channel_id, "", "", None);
    handle_ui_event(&mut state, cleared, &links).await;
    let sent = drain(&mut commands);
    assert_cleared(&fixture, &state, &site, &sent);
}

/// What a refused project change of `#site` must leave as it was.
struct Frozen {
    channel: aura_store_db::ChannelRecord,
    moved: [Placements; 2],
    hidden: HashSet<AgentId>,
    kernel: Arc<Kernel>,
}

/// A change of `#site`'s project was refused as its coordinator is busy,
/// and nothing moved or reopened.
fn assert_refused(
    fixture: &Fixture,
    state: &LoopState<'_>,
    site: &Site,
    frozen: &Frozen,
    sent: &[(Option<String>, UiCommand)],
) {
    let members = [site.coordinator, site.member];
    assert_eq!(
        errors(sent),
        ["#site's coordinator is busy; change the project when it is idle"]
    );
    assert!(!sent_sandbox_root(sent), "{sent:?}");
    assert_eq!(record(fixture, &site.channel_id), frozen.channel);
    assert_eq!(
        members.map(|agent| placements(fixture, agent)),
        frozen.moved
    );
    assert_eq!(*subagents::lock(&fixture.hub.hidden_members), frozen.hidden);
    assert!(
        Arc::ptr_eq(&state.idle[&site.coordinator].kernel, &frozen.kernel),
        "the session is not reopened"
    );
}

/// After a refused switch, `#site`'s members are still listed under A and
/// the coordinator still works in A's folder.
fn assert_still_in_a(fixture: &Fixture, site: &Site, frozen: &Frozen) {
    let [UiCommand::SetProjects(projects), UiCommand::SetAgents(rows), _] =
        fixture.hub.agent_lists()
    else {
        panic!("the lists come as projects, agents, archive");
    };
    assert_eq!(listed(&projects, &site.a), Some(3), "still listed under A");
    assert_eq!(listed(&projects, &site.b), Some(1));
    for agent in [site.coordinator, site.member] {
        assert_eq!(listed_under(&rows, agent), Some(Some(site.a.clone())));
    }
    assert_eq!(frozen.kernel.agent_workspace(), site.a_folder);
}

/// A rename keeping project A went through while the coordinator is busy,
/// without moving or reopening anyone.
fn assert_renamed_in_a(
    fixture: &Fixture,
    state: &LoopState<'_>,
    site: &Site,
    frozen: &Frozen,
    sent: &[(Option<String>, UiCommand)],
) {
    assert_eq!(errors(sent), Vec::<String>::new());
    let renamed = record(fixture, &site.channel_id);
    assert_eq!(
        (renamed.title.as_str(), renamed.description.as_str()),
        ("Relaunch", "New plan.")
    );
    assert_eq!(renamed.project_id.as_deref(), Some(site.a.as_str()));
    assert_eq!(
        [site.coordinator, site.member].map(|agent| placements(fixture, agent)),
        frozen.moved
    );
    assert!(Arc::ptr_eq(
        &state.idle[&site.coordinator].kernel,
        &frozen.kernel
    ));
    let (_, rows, _) = last_lists(sent);
    assert!(
        rows.is_some(),
        "the agent list is sent after any update: {sent:?}"
    );
}

#[tokio::test]
async fn a_busy_coordinator_refuses_a_project_switch_without_moving_or_reopening_anyone() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let site = site_in_a(&fixture, &mut state, &mut commands, &links).await;
    let (coordinator, member) = (site.coordinator, site.member);
    // Mid-turn, with its session still idle: an accepted change would
    // reopen it, as the coordinator is the active agent.
    let kernel = Arc::clone(&state.idle[&coordinator].kernel);
    state.hub.start_running(
        coordinator,
        Arc::clone(&kernel),
        CancellationToken::new(),
        None,
    );
    let frozen = Frozen {
        channel: record(&fixture, &site.channel_id),
        moved: [coordinator, member].map(|agent| placements(&fixture, agent)),
        hidden: subagents::lock(&fixture.hub.hidden_members).clone(),
        kernel,
    };

    let to_b = update(&site.channel_id, "", "", Some(&site.b));
    handle_ui_event(&mut state, to_b, &links).await;
    let sent = drain(&mut commands);
    assert_refused(&fixture, &state, &site, &frozen, &sent);
    assert_still_in_a(&fixture, &site, &frozen);

    // Clearing the project is refused too, before the channel's folder
    // would be made.
    let cleared = update(&site.channel_id, "", "", None);
    handle_ui_event(&mut state, cleared, &links).await;
    let sent = drain(&mut commands);
    assert_refused(&fixture, &state, &site, &frozen, &sent);
    let channel_folder = channel_folder(&fixture, "site");
    assert!(!channel_folder.exists(), "{}", channel_folder.display());

    // Keeping the project, the title and description still change.
    let renamed = update(&site.channel_id, "Relaunch", "New plan.", Some(&site.a));
    handle_ui_event(&mut state, renamed, &links).await;
    let sent = drain(&mut commands);
    assert_renamed_in_a(&fixture, &state, &site, &frozen, &sent);
    state.hub.stop_running(coordinator);
}
