//! Changing a channel's project through the UI (`UiEvent::UpdateChannel`)
//! with members spawned in it: what moves, what the UI is sent, and what
//! a busy coordinator refuses. Also the project a channel works in, which
//! cannot be removed from under it.

use super::support::open_project;
use super::*;
use crate::event_loop::subagents::tests::request;
use aura_ui::events::{AgentSummary, ChannelSummary, ProjectSummary};
use std::path::PathBuf;

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

/// Make channel `#zode` without a project through the UI and open it;
/// returns its id and coordinator, the active agent now.
async fn open_zode(
    state: &mut LoopState<'_>,
    commands: &mut mpsc::Receiver<UiCommand>,
    links: &mpsc::Sender<auth::LinkOutcome>,
) -> (String, AgentId) {
    let create = UiEvent::CreateChannel {
        slug: "zode".to_string(),
        title: "Zode".to_string(),
        description: String::new(),
        project_id: None,
    };
    handle_ui_event(state, create, links).await;
    let _ = drain(commands);
    let channel = state
        .hub
        .channels
        .as_ref()
        .unwrap()
        .by_slug("zode")
        .unwrap()
        .unwrap();
    assert_eq!(state.active, channel.coordinator);
    assert_eq!(state.open_channel.as_deref(), Some(channel.id.as_str()));
    (channel.id, channel.coordinator)
}

fn update(channel_id: &str, title: &str, description: &str, project_id: Option<&str>) -> UiEvent {
    UiEvent::UpdateChannel {
        channel_id: channel_id.to_string(),
        title: title.to_string(),
        description: description.to_string(),
        project_id: project_id.map(str::to_string),
    }
}

fn record(fixture: &Fixture, channel_id: &str) -> aura_store_db::ChannelRecord {
    (fixture.hub.channels.as_ref().unwrap().get(channel_id))
        .unwrap()
        .unwrap()
}

/// The project and own folder of `agent`, in the registry and in its
/// stored row.
type Placements = [(Option<String>, Option<PathBuf>); 2];

fn placements(fixture: &Fixture, agent: AgentId) -> Placements {
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

fn errors(sent: &[(Option<String>, UiCommand)]) -> Vec<String> {
    sent.iter()
        .filter_map(|(_, c)| match c {
            UiCommand::ShowError(problem) => Some(problem.clone()),
            _ => None,
        })
        .collect()
}

/// Whether `sent` shows the tasks of channel `channel_id`.
fn sent_channel_tasks(sent: &[(Option<String>, UiCommand)], channel_id: &str) -> bool {
    sent.iter().any(|(_, c)| {
        matches!(
            c,
            UiCommand::SetChannelTasks { channel_id: id, .. } if id == channel_id
        )
    })
}

/// Whether `sent` points the Sandbox panel anywhere.
fn sent_sandbox_root(sent: &[(Option<String>, UiCommand)]) -> bool {
    sent.iter()
        .any(|(_, c)| matches!(c, UiCommand::SetSandboxRoot(_)))
}

#[tokio::test]
async fn switching_a_channels_project_in_the_ui_moves_its_members_and_reopens_the_coordinator_there(
) {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let site = site_in_a(&fixture, &mut state, &mut commands, &links).await;
    let (coordinator, member) = (site.coordinator, site.member);
    let before = Arc::clone(&state.idle[&coordinator].kernel);
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
    assert_eq!(errors(&sent), Vec::<String>::new());
    assert_eq!(
        record(&fixture, &site.channel_id).project_id.as_deref(),
        Some(site.b.as_str())
    );
    let session = &state.idle[&coordinator];
    assert!(!Arc::ptr_eq(&session.kernel, &before), "a new session");
    assert_eq!(session.kernel.agent_workspace(), site.b_folder);
    assert!(
        sent.iter()
            .any(|(_, c)| matches!(c, UiCommand::SetSandboxRoot(root) if *root == site.b_folder)),
        "{sent:?}"
    );
    // The open channel's tasks are read again from where it works now.
    assert!(sent_channel_tasks(&sent, &site.channel_id), "{sent:?}");
    let (projects, rows, channels) = last_lists(&sent);
    let (projects, rows) = (
        projects.expect("the projects are sent"),
        rows.expect("the agent list is sent"),
    );
    for agent in [coordinator, member] {
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

    // B to none: the members leave the Projects tree and the coordinator
    // works in the channel's folder.
    let cleared = update(&site.channel_id, "", "", None);
    handle_ui_event(&mut state, cleared, &links).await;
    let sent = drain(&mut commands);
    assert_eq!(errors(&sent), Vec::<String>::new());
    assert_eq!(record(&fixture, &site.channel_id).project_id, None);
    let channel_folder = (subagents::lock(&fixture.hub.registry).data_dir())
        .join("channels")
        .join("site");
    assert_eq!(
        state.idle[&coordinator].kernel.agent_workspace(),
        channel_folder
    );
    assert!(
        sent.iter()
            .any(|(_, c)| matches!(c, UiCommand::SetSandboxRoot(root) if *root == channel_folder)),
        "{sent:?}"
    );
    assert!(sent_channel_tasks(&sent, &site.channel_id), "{sent:?}");
    let (_, rows, channels) = last_lists(&sent);
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
    let channel = record(&fixture, &site.channel_id);
    let moved = [coordinator, member].map(|agent| placements(&fixture, agent));
    let hidden = subagents::lock(&fixture.hub.hidden_members).clone();

    let to_b = update(&site.channel_id, "", "", Some(&site.b));
    handle_ui_event(&mut state, to_b, &links).await;
    let sent = drain(&mut commands);
    assert_eq!(
        errors(&sent),
        ["#site's coordinator is busy; change the project when it is idle"]
    );
    assert!(!sent_sandbox_root(&sent), "{sent:?}");
    assert_eq!(record(&fixture, &site.channel_id), channel);
    assert_eq!(
        [coordinator, member].map(|agent| placements(&fixture, agent)),
        moved
    );
    assert_eq!(*subagents::lock(&fixture.hub.hidden_members), hidden);
    let [UiCommand::SetProjects(projects), UiCommand::SetAgents(rows), _] =
        fixture.hub.agent_lists()
    else {
        panic!("the lists come as projects, agents, archive");
    };
    assert_eq!(listed(&projects, &site.a), Some(3), "still listed under A");
    assert_eq!(listed(&projects, &site.b), Some(1));
    for agent in [coordinator, member] {
        assert_eq!(listed_under(&rows, agent), Some(Some(site.a.clone())));
    }
    assert!(
        Arc::ptr_eq(&state.idle[&coordinator].kernel, &kernel),
        "the session is not reopened"
    );
    assert_eq!(kernel.agent_workspace(), site.a_folder);

    // Clearing the project is refused too, before the channel's folder
    // would be made.
    let cleared = update(&site.channel_id, "", "", None);
    handle_ui_event(&mut state, cleared, &links).await;
    let sent = drain(&mut commands);
    assert_eq!(
        errors(&sent),
        ["#site's coordinator is busy; change the project when it is idle"]
    );
    assert!(!sent_sandbox_root(&sent), "{sent:?}");
    assert_eq!(record(&fixture, &site.channel_id), channel);
    assert_eq!(
        [coordinator, member].map(|agent| placements(&fixture, agent)),
        moved
    );
    assert_eq!(*subagents::lock(&fixture.hub.hidden_members), hidden);
    assert!(Arc::ptr_eq(&state.idle[&coordinator].kernel, &kernel));
    let channel_folder = (subagents::lock(&fixture.hub.registry).data_dir())
        .join("channels")
        .join("site");
    assert!(!channel_folder.exists(), "{}", channel_folder.display());

    // Keeping the project, the title and description still change.
    let renamed = update(&site.channel_id, "Relaunch", "New plan.", Some(&site.a));
    handle_ui_event(&mut state, renamed, &links).await;
    let sent = drain(&mut commands);
    assert_eq!(errors(&sent), Vec::<String>::new());
    let renamed = record(&fixture, &site.channel_id);
    assert_eq!(
        (renamed.title.as_str(), renamed.description.as_str()),
        ("Relaunch", "New plan.")
    );
    assert_eq!(renamed.project_id.as_deref(), Some(site.a.as_str()));
    assert_eq!(
        [coordinator, member].map(|agent| placements(&fixture, agent)),
        moved
    );
    assert!(Arc::ptr_eq(&state.idle[&coordinator].kernel, &kernel));
    let (_, rows, _) = last_lists(&sent);
    assert!(
        rows.is_some(),
        "the agent list is sent after any update: {sent:?}"
    );
    state.hub.stop_running(coordinator);
}

/// The other direction from the test above: a channel without a project
/// is refused one, and a rename that keeps no project goes through, which
/// clears to the channel's folder rather than a project's.
#[tokio::test]
async fn a_busy_coordinator_refuses_a_project_for_a_channel_without_one() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let dir = TempDir::new().unwrap();
    let (project_id, _) = open_project(&fixture.hub, dir.path());
    let (channel_id, coordinator) = open_zode(&mut state, &mut commands, &links).await;
    let kernel = Arc::clone(&state.idle[&coordinator].kernel);
    state.hub.start_running(
        coordinator,
        Arc::clone(&kernel),
        CancellationToken::new(),
        None,
    );
    let channel = record(&fixture, &channel_id);
    let placed = placements(&fixture, coordinator);

    let change = update(&channel_id, "Zode", "", Some(&project_id));
    handle_ui_event(&mut state, change, &links).await;
    let sent = drain(&mut commands);
    assert_eq!(
        errors(&sent),
        ["#zode's coordinator is busy; change the project when it is idle"]
    );
    assert!(!sent_sandbox_root(&sent), "{sent:?}");
    assert_eq!(record(&fixture, &channel_id), channel);
    assert_eq!(channel.project_id, None);
    assert_eq!(placements(&fixture, coordinator), placed);
    assert!(
        Arc::ptr_eq(&state.idle[&coordinator].kernel, &kernel),
        "the session is not reopened"
    );

    let renamed = update(&channel_id, "Renamed", "", None);
    handle_ui_event(&mut state, renamed, &links).await;
    let sent = drain(&mut commands);
    assert_eq!(errors(&sent), Vec::<String>::new());
    let renamed = record(&fixture, &channel_id);
    assert_eq!(
        (renamed.title.as_str(), renamed.project_id),
        ("Renamed", None)
    );
    state.hub.stop_running(coordinator);
}

#[tokio::test]
async fn a_project_change_reopens_the_coordinator_in_the_new_folder() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let dir = TempDir::new().unwrap();
    let (project_id, project_folder) = open_project(&fixture.hub, dir.path());
    let (channel_id, coordinator) = open_zode(&mut state, &mut commands, &links).await;
    let before = state.idle[&coordinator].kernel.agent_workspace();
    assert!(!before.starts_with(&project_folder), "{}", before.display());

    let change = update(&channel_id, "Zode", "", Some(&project_id));
    handle_ui_event(&mut state, change, &links).await;
    let sent = drain(&mut commands);
    assert!(
        !sent
            .iter()
            .any(|(_, c)| matches!(c, UiCommand::ShowError(_))),
        "{sent:?}"
    );
    // The next turn runs in the project, not in the session from before.
    let after = state.idle[&coordinator].kernel.agent_workspace();
    assert!(after.starts_with(&project_folder), "{}", after.display());
    assert!(
        sent.iter()
            .any(|(_, c)| matches!(c, UiCommand::SetSandboxRoot(root) if *root == after)),
        "{sent:?}"
    );
    // The open channel's tasks are read again from where it works now.
    assert!(
        sent.iter().any(|(_, c)| matches!(
            c,
            UiCommand::SetChannelTasks { channel_id: id, .. } if *id == channel_id
        )),
        "{sent:?}"
    );
}

#[tokio::test]
async fn a_project_a_channel_works_in_cannot_be_removed() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let dir = TempDir::new().unwrap();
    let (project_id, _) = open_project(&fixture.hub, dir.path());

    let create = UiEvent::CreateChannel {
        slug: "zode".to_string(),
        title: "Zode".to_string(),
        description: String::new(),
        project_id: Some(project_id.clone()),
    };
    handle_ui_event(&mut state, create, &links).await;
    let _ = drain(&mut commands);

    handle_ui_event(
        &mut state,
        UiEvent::RemoveProject(project_id.clone()),
        &links,
    )
    .await;
    let sent = drain(&mut commands);
    assert!(
        sent.iter().any(|(_, c)| matches!(
            c,
            UiCommand::ShowError(problem)
                if problem.contains("#zode works in this project; edit the channel")
        )),
        "{sent:?}"
    );
    let registry = subagents::lock(&fixture.hub.registry);
    assert!(registry.project(&project_id).is_some_and(|p| !p.removed));
    assert!(registry
        .agents()
        .iter()
        .any(|a| a.project_id.as_deref() == Some(project_id.as_str())));
}
