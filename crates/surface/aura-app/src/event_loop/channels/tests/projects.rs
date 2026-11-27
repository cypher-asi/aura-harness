//! Channels working in a project: creating, setting and clearing it.

use super::super::*;
use super::support::{
    agent_count, edit, is_listed, new_channel, new_channel_in, placement, projects_tree,
    spawn_under, store, stored_placement, summary_of, unpin,
};
use crate::event_loop::subagents::tests::{fixture, Fixture};
use crate::event_loop::tests::support::open_project;
use std::path::Path;

#[tokio::test]
async fn a_channel_with_a_project_puts_its_coordinator_there_without_a_channel_folder() {
    let fixture = fixture("unused");
    let dir = tempfile::TempDir::new().unwrap();
    let (project_id, project_folder) = open_project(&fixture.hub, dir.path());
    let channel = create_channel(&fixture.hub, new_channel_in("site", &project_id))
        .await
        .unwrap();
    assert_eq!(channel.project_id.as_deref(), Some(project_id.as_str()));

    assert_eq!(
        placement(&fixture, channel.coordinator),
        (Some(project_id.clone()), None)
    );
    assert_eq!(
        stored_placement(&fixture, channel.coordinator),
        (Some(project_id.clone()), None)
    );
    let registry = lock(&fixture.hub.registry);
    assert_eq!(
        registry.project_folder(channel.coordinator),
        Some(project_folder)
    );
    assert!(!registry.data_dir().join("channels").join("site").exists());
    drop(registry);
    assert!(lock(&fixture.hub.hidden_members).is_empty());
}

#[tokio::test]
async fn a_channel_with_an_unknown_project_is_refused_without_registering_anyone() {
    let fixture = fixture("unused");
    let before = agent_count(&fixture);
    let error = create_channel(&fixture.hub, new_channel_in("site", "no-such-project"))
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("no project"), "{error:#}");
    assert_eq!(agent_count(&fixture), before);
    assert!(store(&fixture).list().unwrap().is_empty());
    assert!(!lock(&fixture.hub.registry)
        .data_dir()
        .join("channels")
        .join("site")
        .exists());
}

/// `[coordinator, agent, subagent]`, each spawned by the one before, are
/// all listed under `project_id`, each a level deeper than its parent.
fn assert_nested_under_project(fixture: &Fixture, project_id: &str, members: [AgentId; 3]) {
    let [coordinator, agent, subagent] = members;
    let (rows, projects) = projects_tree(fixture);
    assert!(projects.contains(&project_id.to_string()), "{projects:?}");
    for member in members {
        let row = rows
            .iter()
            .find(|row| row.id == member.to_hex())
            .unwrap_or_else(|| panic!("{} is listed", member.to_hex()));
        assert_eq!(row.project_id.as_deref(), Some(project_id));
    }
    let depth_of = |member: AgentId| {
        rows.iter()
            .find(|row| row.id == member.to_hex())
            .map(|row| row.depth)
    };
    assert_eq!(depth_of(agent), depth_of(coordinator).map(|d| d + 1));
    assert_eq!(depth_of(subagent), depth_of(agent).map(|d| d + 1));
    assert!(fixture
        .hub
        .agent_summaries()
        .iter()
        .any(|row| row.id == subagent.to_hex()));
}

#[tokio::test]
async fn spawns_of_a_projected_channel_work_in_its_project_and_are_listed_under_it() {
    let mut fixture = fixture("Looks good.");
    let dir = tempfile::TempDir::new().unwrap();
    let (project_id, project_folder) = open_project(&fixture.hub, dir.path());
    let channel = create_channel(&fixture.hub, new_channel_in("site", &project_id))
        .await
        .unwrap();
    let agent = spawn_under(&mut fixture, channel.coordinator).await;
    let subagent = spawn_under(&mut fixture, agent).await;

    for member in [agent, subagent] {
        let pinned = (Some(project_id.clone()), Some(project_folder.clone()));
        assert_eq!(placement(&fixture, member), pinned, "{}", member.to_hex());
        assert_eq!(stored_placement(&fixture, member), pinned);
        assert_eq!(
            store(&fixture).channel_of(&member).unwrap().as_deref(),
            Some(channel.id.as_str())
        );
    }

    let members = [channel.coordinator, agent, subagent];
    assert_nested_under_project(&fixture, &project_id, members);

    let summary = summary_of(&channel_summaries(&fixture.hub).await.unwrap(), &channel.id).unwrap();
    let ids: Vec<String> = summary.members.iter().map(|m| m.id.clone()).collect();
    assert_eq!(ids, members.map(|member| member.to_hex()));
}

/// The channel now works in `project_id`: its coordinator is there with no
/// folder of its own, the earlier member `agent` is there too, still
/// working in `channel_folder`, and both are listed.
fn assert_moved_into_project(
    fixture: &Fixture,
    [coordinator, agent]: [AgentId; 2],
    (project_id, project_folder): (&str, PathBuf),
    channel_folder: &Path,
) {
    for check in [placement, stored_placement] {
        assert_eq!(
            check(fixture, coordinator),
            (Some(project_id.to_string()), None)
        );
        assert_eq!(
            check(fixture, agent),
            (
                Some(project_id.to_string()),
                Some(channel_folder.to_path_buf())
            ),
            "the earlier member is in the new project, still working where it did"
        );
    }
    assert_eq!(
        lock(&fixture.hub.registry).project_folder(coordinator),
        Some(project_folder)
    );
    let (rows, _) = projects_tree(fixture);
    assert!(is_listed(&rows, coordinator));
    assert!(is_listed(&rows, agent));
    assert!(lock(&fixture.hub.hidden_members).is_empty());
}

/// The channel works in no project again: both members are back in
/// `launch_project`, working in `channel_folder`, and hidden.
fn assert_back_in_channel_folder(
    fixture: &Fixture,
    [coordinator, agent]: [AgentId; 2],
    launch_project: Option<&str>,
    channel_folder: &Path,
) {
    let launch_project = launch_project.map(str::to_string);
    for check in [placement, stored_placement] {
        assert_eq!(
            check(fixture, coordinator),
            (launch_project.clone(), Some(channel_folder.to_path_buf()))
        );
        assert_eq!(
            check(fixture, agent),
            (launch_project.clone(), Some(channel_folder.to_path_buf()))
        );
    }
    let (rows, _) = projects_tree(fixture);
    assert!(!is_listed(&rows, coordinator));
    assert!(!is_listed(&rows, agent));
    let hidden: HashSet<_> = [coordinator, agent].into();
    assert_eq!(*lock(&fixture.hub.hidden_members), hidden);
}

#[tokio::test]
async fn setting_and_clearing_a_project_moves_the_coordinator_and_keeps_earlier_work_in_place() {
    let mut fixture = fixture("Looks good.");
    let dir = tempfile::TempDir::new().unwrap();
    let (project_id, project_folder) = open_project(&fixture.hub, dir.path());
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let agent = spawn_under(&mut fixture, channel.coordinator).await;
    let (launch_project, channel_folder) = placement(&fixture, channel.coordinator);
    let channel_folder = channel_folder.unwrap();
    // A member from before folders were pinned on joining.
    unpin(&fixture, agent, launch_project.clone());

    let to_project = edit("", "Ship it.", Some(project_id.clone()));
    update_channel(&fixture.hub, &channel.id, to_project)
        .await
        .unwrap();
    let record = store(&fixture).get(&channel.id).unwrap().unwrap();
    assert_eq!(record.project_id.as_deref(), Some(project_id.as_str()));
    assert_eq!(record.title, "Website relaunch");
    let members = [channel.coordinator, agent];
    let project = (project_id.as_str(), project_folder);
    assert_moved_into_project(&fixture, members, project, &channel_folder);

    update_channel(&fixture.hub, &channel.id, edit("", "Ship it.", None))
        .await
        .unwrap();
    let record = store(&fixture).get(&channel.id).unwrap().unwrap();
    assert_eq!(record.project_id, None);
    assert_back_in_channel_folder(
        &fixture,
        members,
        launch_project.as_deref(),
        &channel_folder,
    );
}

#[tokio::test]
async fn an_unknown_project_is_refused_and_a_title_change_keeps_the_project() {
    let fixture = fixture("unused");
    let dir = tempfile::TempDir::new().unwrap();
    let (project_id, _) = open_project(&fixture.hub, dir.path());
    let channel = create_channel(&fixture.hub, new_channel_in("site", &project_id))
        .await
        .unwrap();
    let before = placement(&fixture, channel.coordinator);

    let unknown = edit("Other", "Other.", Some("no-such-project".to_string()));
    let error = update_channel(&fixture.hub, &channel.id, unknown)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("no project"), "{error:#}");
    assert_eq!(
        store(&fixture).get(&channel.id).unwrap().as_ref(),
        Some(&channel)
    );
    assert_eq!(placement(&fixture, channel.coordinator), before);
    assert_eq!(stored_placement(&fixture, channel.coordinator), before);

    let retitle = edit("Relaunch", "New plan.", Some(project_id.clone()));
    update_channel(&fixture.hub, &channel.id, retitle)
        .await
        .unwrap();
    let record = store(&fixture).get(&channel.id).unwrap().unwrap();
    assert_eq!(record.title, "Relaunch");
    assert_eq!(record.project_id.as_deref(), Some(project_id.as_str()));
    assert_eq!(placement(&fixture, channel.coordinator), before);
}

#[tokio::test]
async fn a_failed_project_change_leaves_the_registry_as_it_was() {
    let mut fixture = fixture("Looks good.");
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let agent = spawn_under(&mut fixture, channel.coordinator).await;
    let before = placement(&fixture, channel.coordinator);
    let moved = lock(&fixture.hub.registry).place_agents(
        &[AgentPlacement {
            agent_id: channel.coordinator,
            project_id: Some("elsewhere".to_string()),
            folder: None,
        }],
        |_, _| -> anyhow::Result<()> { bail!("the disk is full") },
    );
    assert!(moved.is_err());
    assert_eq!(placement(&fixture, channel.coordinator), before);
    assert_eq!(stored_placement(&fixture, channel.coordinator), before);
    // The next save does not write the placement that failed either.
    lock(&fixture.hub.registry)
        .set_title(agent, "Something else")
        .unwrap();
    assert_eq!(stored_placement(&fixture, channel.coordinator), before);
}
