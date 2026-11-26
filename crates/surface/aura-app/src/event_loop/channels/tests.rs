use super::*;
use crate::event_loop::subagents::task_tool_definition;
use crate::event_loop::subagents::tests::{fixture, request, Fixture};
use crate::event_loop::tests::support::open_project;
use aura_ui::UiCommand;

fn new_channel(slug: &str) -> NewChannel {
    NewChannel {
        slug: slug.to_string(),
        title: "Website relaunch".to_string(),
        description: "Ship the new marketing site.".to_string(),
        project_id: None,
    }
}

/// [`new_channel`] working in project `project_id`.
fn new_channel_in(slug: &str, project_id: &str) -> NewChannel {
    NewChannel {
        project_id: Some(project_id.to_string()),
        ..new_channel(slug)
    }
}

/// The project and own folder `agent` has in the registry.
fn placement(fixture: &Fixture, agent: AgentId) -> (Option<String>, Option<PathBuf>) {
    let registry = lock(&fixture.hub.registry);
    let found = registry.find(agent).unwrap();
    (found.project_id.clone(), found.folder.clone())
}

/// The stored row of `agent`: its project and own folder.
fn stored_placement(fixture: &Fixture, agent: AgentId) -> (Option<String>, Option<PathBuf>) {
    let rows = lock(&fixture.hub.registry).store().load().unwrap().agents;
    let row = rows.iter().find(|row| row.agent_id == agent).unwrap();
    (row.project_id.clone(), row.folder.clone())
}

/// The ids in the Agents panel rows and the ids of the projects listed.
fn projects_tree(fixture: &Fixture) -> (Vec<aura_ui::events::AgentSummary>, Vec<String>) {
    let [UiCommand::SetProjects(projects), UiCommand::SetAgents(rows), _] =
        fixture.hub.agent_lists()
    else {
        panic!("the lists come as projects, agents, archive");
    };
    (rows, projects.into_iter().map(|p| p.id).collect())
}

fn is_listed(rows: &[aura_ui::events::AgentSummary], agent: AgentId) -> bool {
    rows.iter().any(|row| row.id == agent.to_hex())
}

/// Clear `agent`'s own folder, in memory and stored, keeping it in
/// `project_id`, as a member that joined before folders were pinned.
fn unpin(fixture: &Fixture, agent: AgentId, project_id: Option<String>) {
    let unpinned = AgentPlacement {
        agent_id: agent,
        project_id,
        folder: None,
    };
    lock(&fixture.hub.registry)
        .place_agents(&[unpinned], |agents, rows| {
            let mut batch = WriteBatch::default();
            stage_rows(agents, rows, &mut batch)?;
            Ok(store(fixture).write(batch)?)
        })
        .unwrap();
    assert_eq!(stored_placement(fixture, agent).1, None);
}

fn agent_count(fixture: &Fixture) -> usize {
    lock(&fixture.hub.registry).agents().len()
}

fn store(fixture: &Fixture) -> &ChannelStore {
    fixture.hub.channels.as_ref().unwrap()
}

#[tokio::test]
async fn create_channel_writes_the_channel_and_its_coordinator_together() {
    let fixture = fixture("unused");
    let channel = create_channel(&fixture.hub, new_channel("  site  "))
        .await
        .unwrap();
    assert_eq!(channel.slug, "site");

    let store = store(&fixture);
    assert_eq!(store.get(&channel.id).unwrap().as_ref(), Some(&channel));
    assert_eq!(store.by_slug("site").unwrap().as_ref(), Some(&channel));

    let registry = lock(&fixture.hub.registry);
    let coordinator = registry.find(channel.coordinator).unwrap().clone();
    let folder = registry.data_dir().join("channels").join("site");
    assert_eq!(
        coordinator.definition.as_deref(),
        Some(COORDINATOR_DEFINITION)
    );
    assert_eq!(coordinator.name, "#site coordinator");
    assert!(coordinator.code.is_some());
    assert!(coordinator.project_id.is_some());
    assert_eq!(coordinator.folder.as_deref(), Some(folder.as_path()));
    assert!(folder.is_dir());

    let members = store.members(&channel.id).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].agent_id, channel.coordinator);
    assert_eq!(members[0].role, MemberRole::Coordinator);

    let rows = registry.store().load().unwrap().agents;
    let row = rows
        .iter()
        .find(|row| row.agent_id == channel.coordinator)
        .unwrap();
    assert_eq!(row.code, coordinator.code);
}

#[tokio::test]
async fn invalid_and_taken_slugs_are_rejected_without_registering_anyone() {
    let fixture = fixture("unused");
    create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let before = agent_count(&fixture);

    for slug in ["", "Not Valid", "under_score", &"a".repeat(41)] {
        let error = create_channel(&fixture.hub, new_channel(slug))
            .await
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("invalid channel name"),
            "{slug}: {error:#}"
        );
    }
    let error = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("already exists"), "{error:#}");

    assert_eq!(agent_count(&fixture), before);
    assert_eq!(store(&fixture).list().unwrap().len(), 1);
}

#[tokio::test]
async fn channel_prompt_describes_the_channel_to_its_coordinator_only() {
    let fixture = fixture("unused");
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let code = lock(&fixture.hub.registry)
        .find(channel.coordinator)
        .and_then(|a| a.code.clone())
        .unwrap();

    let prompt = channel_prompt(&fixture.hub, channel.coordinator).unwrap();
    assert!(prompt.starts_with("<channel>"), "{prompt}");
    assert!(prompt.contains("#site"), "{prompt}");
    assert!(prompt.contains("Website relaunch"), "{prompt}");
    assert!(prompt.contains("Ship the new marketing site."), "{prompt}");
    assert!(prompt.contains(&format!("- {code} ")), "{prompt}");
    assert!(prompt.contains("(coordinator)"), "{prompt}");

    let outsider = lock(&fixture.hub.registry).active().agent_id();
    assert_eq!(channel_prompt(&fixture.hub, outsider), None);
}

#[test]
fn the_task_tool_never_offers_the_coordinator() {
    let fixture = fixture("unused");
    let definitions = lock(&fixture.hub.definitions);
    assert!(definitions.get(COORDINATOR_DEFINITION).is_some());
    let tool = task_tool_definition(&definitions).unwrap();
    let names = &tool.input_schema["properties"]["subagent_type"]["enum"];
    assert!(
        !names
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == COORDINATOR_DEFINITION),
        "{names}"
    );
    drop(definitions);
    assert!(fixture
        .hub
        .definition_summaries()
        .iter()
        .all(|summary| summary.name != COORDINATOR_DEFINITION));
}

#[test]
fn coordinator_turns_can_delegate_but_not_edit() {
    let fixture = fixture("unused");
    let workspace = lock(&fixture.hub.registry).data_dir().to_path_buf();
    let (_, tools) = fixture.hub.turn(Some(COORDINATOR_DEFINITION), &workspace);
    assert!(tools.iter().any(|t| t.name == "task"));
    assert!(tools
        .iter()
        .all(|t| t.name != "write_file" && t.name != "edit_file" && t.name != "run_command"));
}

/// Spawn a reviewer under `parent` through the real `task` dispatch path;
/// returns the child. Drains the UI commands the spawn sent: each one
/// sends a few dozen and nothing else reads the fixture's 64-slot
/// channel, so a third undrained spawn would wait on it forever.
async fn spawn_under(fixture: &mut Fixture, parent: AgentId) -> AgentId {
    let child = fixture
        .dispatcher
        .dispatch(request(parent, "reviewer", "Review the diff"))
        .await
        .unwrap()
        .child_agent_id
        .unwrap();
    fixture.drain_commands();
    child
}

#[tokio::test]
async fn agents_spawned_in_a_channel_join_it_at_every_depth() {
    let mut fixture = fixture("Looks good.");
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let agent = spawn_under(&mut fixture, channel.coordinator).await;
    let subagent = spawn_under(&mut fixture, agent).await;
    let outsider_parent = lock(&fixture.hub.registry).active().agent_id();
    let outsider = spawn_under(&mut fixture, outsider_parent).await;

    let store = store(&fixture);
    let members: Vec<_> = store
        .members(&channel.id)
        .unwrap()
        .into_iter()
        .map(|m| (m.agent_id, m.role, m.parent))
        .collect();
    assert_eq!(
        members,
        [
            (channel.coordinator, MemberRole::Coordinator, None),
            (agent, MemberRole::Agent, Some(channel.coordinator)),
            (subagent, MemberRole::Subagent, Some(agent)),
        ]
    );
    for member in [channel.coordinator, agent, subagent] {
        assert_eq!(
            store.channel_of(&member).unwrap().as_deref(),
            Some(channel.id.as_str())
        );
    }
    assert_eq!(store.channel_of(&outsider).unwrap(), None);
    assert_eq!(store.channel_of(&outsider_parent).unwrap(), None);

    // The agent rows were written with the member rows.
    let rows = lock(&fixture.hub.registry).store().load().unwrap().agents;
    assert!(rows.iter().any(|row| row.agent_id == subagent));

    // The in-memory set matches the store, as a restart would load it.
    let expected: HashSet<_> = [channel.coordinator, agent, subagent].into();
    assert_eq!(*lock(&fixture.hub.channel_members), expected);
    assert_eq!(load_members(Some(store)), expected);
}

#[tokio::test]
async fn channel_members_are_left_out_of_the_projects_tree() {
    let mut fixture = fixture("Looks good.");
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let agent = spawn_under(&mut fixture, channel.coordinator).await;
    let subagent = spawn_under(&mut fixture, agent).await;
    let (outsider, coordinator_project) = {
        let registry = lock(&fixture.hub.registry);
        let project = registry
            .find(channel.coordinator)
            .and_then(|a| a.project_id.clone())
            .unwrap();
        (registry.active().agent_id(), project)
    };

    let [UiCommand::SetProjects(projects), UiCommand::SetAgents(rows), _] =
        fixture.hub.agent_lists()
    else {
        panic!("the lists come as projects, agents, archive");
    };
    let listed: HashSet<String> = rows.iter().map(|row| row.id.clone()).collect();
    assert!(listed.contains(&outsider.to_hex()), "{listed:?}");
    for member in [channel.coordinator, agent, subagent] {
        assert!(!listed.contains(&member.to_hex()), "{listed:?}");
    }
    // The coordinator works in the launch project, which the outsider is
    // in too: the project stays listed, counting only the outsider's rows.
    let project = projects
        .iter()
        .find(|p| p.id == coordinator_project)
        .expect("the launch project, shared with the outsider, is listed");
    let in_project = rows
        .iter()
        .filter(|row| row.project_id.as_deref() == Some(coordinator_project.as_str()))
        .count();
    assert!(in_project > 0);
    assert_eq!(project.listed_agents, in_project);

    let summaries = fixture.hub.agent_summaries();
    assert!(summaries.iter().any(|row| row.id == outsider.to_hex()));
    for member in [channel.coordinator, agent, subagent] {
        assert!(summaries.iter().all(|row| row.id != member.to_hex()));
    }
    // Still the channel's members, with rows for its MEMBERS panel.
    let member_rows = fixture.hub.member_rows();
    for member in [channel.coordinator, agent, subagent] {
        assert!(member_rows.contains_key(&member), "{}", member.to_hex());
    }
    let expected: HashSet<_> = [channel.coordinator, agent, subagent].into();
    assert_eq!(*lock(&fixture.hub.hidden_members), expected);
}

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

    let (rows, projects) = projects_tree(&fixture);
    assert!(projects.contains(&project_id), "{projects:?}");
    for member in [channel.coordinator, agent, subagent] {
        let row = rows
            .iter()
            .find(|row| row.id == member.to_hex())
            .unwrap_or_else(|| panic!("{} is listed", member.to_hex()));
        assert_eq!(row.project_id.as_deref(), Some(project_id.as_str()));
    }
    let depth_of = |member: AgentId| {
        rows.iter()
            .find(|row| row.id == member.to_hex())
            .map(|row| row.depth)
    };
    assert_eq!(
        depth_of(agent),
        depth_of(channel.coordinator).map(|d| d + 1)
    );
    assert_eq!(depth_of(subagent), depth_of(agent).map(|d| d + 1));
    assert!(fixture
        .hub
        .agent_summaries()
        .iter()
        .any(|row| row.id == subagent.to_hex()));

    let summary = summary_of(&channel_summaries(&fixture.hub).await.unwrap(), &channel.id).unwrap();
    let ids: Vec<String> = summary.members.iter().map(|m| m.id.clone()).collect();
    assert_eq!(
        ids,
        [
            channel.coordinator.to_hex(),
            agent.to_hex(),
            subagent.to_hex()
        ]
    );
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

    update_channel(
        &fixture.hub,
        &channel.id,
        "",
        "Ship it.",
        Some(project_id.clone()),
    )
    .await
    .unwrap();
    let record = store(&fixture).get(&channel.id).unwrap().unwrap();
    assert_eq!(record.project_id.as_deref(), Some(project_id.as_str()));
    assert_eq!(record.title, "Website relaunch");
    for check in [placement, stored_placement] {
        assert_eq!(
            check(&fixture, channel.coordinator),
            (Some(project_id.clone()), None)
        );
        assert_eq!(
            check(&fixture, agent),
            (Some(project_id.clone()), Some(channel_folder.clone())),
            "the earlier member is in the new project, still working where it did"
        );
    }
    assert_eq!(
        lock(&fixture.hub.registry).project_folder(channel.coordinator),
        Some(project_folder)
    );
    let (rows, _) = projects_tree(&fixture);
    assert!(is_listed(&rows, channel.coordinator));
    assert!(is_listed(&rows, agent));
    assert!(lock(&fixture.hub.hidden_members).is_empty());

    update_channel(&fixture.hub, &channel.id, "", "Ship it.", None)
        .await
        .unwrap();
    let record = store(&fixture).get(&channel.id).unwrap().unwrap();
    assert_eq!(record.project_id, None);
    for check in [placement, stored_placement] {
        assert_eq!(
            check(&fixture, channel.coordinator),
            (launch_project.clone(), Some(channel_folder.clone()))
        );
        assert_eq!(
            check(&fixture, agent),
            (launch_project.clone(), Some(channel_folder.clone()))
        );
    }
    let (rows, _) = projects_tree(&fixture);
    assert!(!is_listed(&rows, channel.coordinator));
    assert!(!is_listed(&rows, agent));
    let hidden: HashSet<_> = [channel.coordinator, agent].into();
    assert_eq!(*lock(&fixture.hub.hidden_members), hidden);
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

    let error = update_channel(
        &fixture.hub,
        &channel.id,
        "Other",
        "Other.",
        Some("no-such-project".to_string()),
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("no project"), "{error:#}");
    assert_eq!(
        store(&fixture).get(&channel.id).unwrap().as_ref(),
        Some(&channel)
    );
    assert_eq!(placement(&fixture, channel.coordinator), before);
    assert_eq!(stored_placement(&fixture, channel.coordinator), before);

    update_channel(
        &fixture.hub,
        &channel.id,
        "Relaunch",
        "New plan.",
        Some(project_id.clone()),
    )
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

#[tokio::test]
async fn start_up_places_coordinators_of_channels_that_got_a_project_from_legacy_links() {
    let mut fixture = fixture("Looks good.");
    let dir = tempfile::TempDir::new().unwrap();
    let (project_id, _) = open_project(&fixture.hub, dir.path());
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let agent = spawn_under(&mut fixture, channel.coordinator).await;
    let (launch_project, channel_folder) = placement(&fixture, channel.coordinator);
    // A member from before folders were pinned on joining.
    unpin(&fixture, agent, launch_project.clone());
    store(&fixture)
        .link_project(&channel.id, &project_id, chrono::Utc::now())
        .unwrap();

    // What the hub runs as it is built at start-up, run twice: the second
    // pass changes nothing.
    for _ in 0..2 {
        let sets = load_channels(Some(store(&fixture)), &mut lock(&fixture.hub.registry));
        assert_eq!(
            sets.members,
            [channel.coordinator, agent].into_iter().collect()
        );
        assert!(sets.hidden.is_empty(), "{:?}", sets.hidden);
        let record = store(&fixture).get(&channel.id).unwrap().unwrap();
        assert_eq!(record.project_id.as_deref(), Some(project_id.as_str()));
        assert!(store(&fixture).projects(&channel.id).unwrap().is_empty());
        for check in [placement, stored_placement] {
            assert_eq!(
                check(&fixture, channel.coordinator),
                (Some(project_id.clone()), None)
            );
            assert_eq!(
                check(&fixture, agent),
                (Some(project_id.clone()), channel_folder.clone())
            );
        }
    }
}

#[tokio::test]
async fn start_up_treats_a_channel_whose_project_is_gone_as_having_none() {
    let mut fixture = fixture("Looks good.");
    let dir = tempfile::TempDir::new().unwrap();
    let (project_id, _) = open_project(&fixture.hub, dir.path());
    let channel = create_channel(&fixture.hub, new_channel_in("site", &project_id))
        .await
        .unwrap();
    let agent = spawn_under(&mut fixture, channel.coordinator).await;
    // The project leaves the registry behind the store's back, as an
    // older build or a hand-edited registry could leave it.
    store(&fixture)
        .update(
            &channel.id,
            ChannelUpdate {
                project_id: Some(Some("gone".to_string())),
                ..ChannelUpdate::default()
            },
            chrono::Utc::now(),
        )
        .unwrap();
    let before = placement(&fixture, channel.coordinator);

    let sets = load_channels(Some(store(&fixture)), &mut lock(&fixture.hub.registry));
    assert_eq!(placement(&fixture, channel.coordinator), before);
    assert_eq!(sets.hidden, [channel.coordinator, agent].into());
    *lock(&fixture.hub.hidden_members) = sets.hidden;
    let (rows, _) = projects_tree(&fixture);
    assert!(!is_listed(&rows, channel.coordinator));

    // INFO shows it as missing, with no folder, so the user can change it.
    let summary = summary_of(&channel_summaries(&fixture.hub).await.unwrap(), &channel.id).unwrap();
    let project = summary.project.unwrap();
    assert_eq!(
        (project.id.as_str(), project.name.as_str()),
        ("gone", MISSING_PROJECT)
    );
    assert!(project.folder.is_empty());
    let prompt = channel_prompt(&fixture.hub, channel.coordinator).unwrap();
    assert!(prompt.contains("Project: missing"), "{prompt}");
}

#[tokio::test]
async fn start_up_migrates_only_links_to_projects_the_registry_lists() {
    let fixture = fixture("unused");
    let dir = tempfile::TempDir::new().unwrap();
    let (project_id, _) = open_project(&fixture.hub, dir.path());
    let removed = tempfile::TempDir::new().unwrap();
    let (removed_id, _) = open_project(&fixture.hub, removed.path());
    lock(&fixture.hub.registry)
        .remove_project(&removed_id)
        .unwrap();
    let linked = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let unlinked = create_channel(&fixture.hub, new_channel("docs"))
        .await
        .unwrap();
    let store = store(&fixture);
    let at = |secs| chrono::DateTime::from_timestamp(secs, 0).unwrap();
    store.link_project(&linked.id, "gone", at(1)).unwrap();
    store.link_project(&linked.id, &removed_id, at(2)).unwrap();
    store.link_project(&linked.id, &project_id, at(3)).unwrap();
    store.link_project(&unlinked.id, "gone", at(1)).unwrap();

    let sets = load_channels(Some(store), &mut lock(&fixture.hub.registry));
    let project_of = |id: &str| store.get(id).unwrap().unwrap().project_id;
    assert_eq!(project_of(&linked.id), Some(project_id.clone()));
    assert_eq!(project_of(&unlinked.id), None);
    for channel in [&linked, &unlinked] {
        assert!(store.projects(&channel.id).unwrap().is_empty());
    }
    assert!(!sets.hidden.contains(&linked.coordinator));
    assert!(sets.hidden.contains(&unlinked.coordinator));
}

fn summary_of(summaries: &[ChannelSummary], id: &str) -> Option<ChannelSummary> {
    summaries.iter().find(|s| s.id == id).cloned()
}

#[tokio::test]
async fn summaries_list_the_coordinator_first_and_nest_spawned_members() {
    let mut fixture = fixture("Looks good.");
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let agent = fixture
        .dispatcher
        .dispatch(request(channel.coordinator, "reviewer", "Review the diff"))
        .await
        .unwrap()
        .child_agent_id
        .unwrap();
    // The spawn refreshed the agent list, and the channels with it.
    let sent = fixture.take_commands();
    assert!(
        sent.iter().any(|c| matches!(
            c,
            UiCommand::SetChannels(list)
                if list.iter().any(|s| s.members.iter().any(|m| m.id == agent.to_hex()))
        )),
        "no SetChannels with the spawned member"
    );

    let summaries = channel_summaries(&fixture.hub).await.unwrap();
    assert_eq!(summaries.len(), 1);
    let summary = &summaries[0];
    assert_eq!(summary.id, channel.id);
    assert_eq!(summary.slug, "site");
    assert_eq!(summary.title, "Website relaunch");
    assert_eq!(summary.coordinator_id, channel.coordinator.to_hex());
    assert!(!summary.archived);
    let members: Vec<(String, usize)> = summary
        .members
        .iter()
        .map(|m| (m.id.clone(), m.depth))
        .collect();
    assert_eq!(
        members,
        [(channel.coordinator.to_hex(), 0), (agent.to_hex(), 1),]
    );
    // Member rows are the Agents panel's rows: named and coded the same.
    let registry = lock(&fixture.hub.registry);
    let child = registry.find(agent).unwrap();
    assert_eq!(summary.members[1].name, child.display_name());
    assert_eq!(summary.members[1].code, child.code);
}

#[tokio::test]
async fn removing_the_launch_project_keeps_channel_members() {
    let mut fixture = fixture("Looks good.");
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let agent = spawn_under(&mut fixture, channel.coordinator).await;
    let other = tempfile::TempDir::new().unwrap();
    let launch_project = {
        let mut registry = lock(&fixture.hub.registry);
        let project = registry
            .find(channel.coordinator)
            .and_then(|a| a.project_id.clone())
            .unwrap();
        // Another project, so the launch project is not the last one.
        registry.open_project(other.path()).unwrap();
        registry.remove_project(&project).unwrap();
        project
    };

    {
        let registry = lock(&fixture.hub.registry);
        assert!(registry.project(&launch_project).is_some_and(|p| p.removed));
        for member in [channel.coordinator, agent] {
            assert!(registry.find(member).is_some(), "{}", member.to_hex());
        }
        let rows = registry.store().load().unwrap().agents;
        for member in [channel.coordinator, agent] {
            assert!(rows.iter().any(|row| row.agent_id == member));
        }
    }
    let members: Vec<AgentId> = store(&fixture)
        .members(&channel.id)
        .unwrap()
        .into_iter()
        .map(|m| m.agent_id)
        .collect();
    assert_eq!(members, [channel.coordinator, agent]);

    // They are still listed, so the channel still shows them.
    let summaries = channel_summaries(&fixture.hub).await.unwrap();
    let ids: Vec<String> = summaries[0].members.iter().map(|m| m.id.clone()).collect();
    assert_eq!(ids, [channel.coordinator.to_hex(), agent.to_hex()]);
}

#[tokio::test]
async fn summaries_and_the_coordinator_prompt_name_the_channel_project() {
    let fixture = fixture("unused");
    let dir = tempfile::TempDir::new().unwrap();
    let (project_id, folder) = open_project(&fixture.hub, dir.path());
    let name = lock(&fixture.hub.registry)
        .project(&project_id)
        .unwrap()
        .name();
    let projected = create_channel(&fixture.hub, new_channel_in("site", &project_id))
        .await
        .unwrap();
    let plain = create_channel(&fixture.hub, new_channel("docs"))
        .await
        .unwrap();

    let summaries = channel_summaries(&fixture.hub).await.unwrap();
    let project = summary_of(&summaries, &projected.id)
        .unwrap()
        .project
        .unwrap();
    assert_eq!(project.id, project_id);
    assert_eq!(project.name, name);
    assert_eq!(project.folder, folder.display().to_string());
    assert_eq!(summary_of(&summaries, &plain.id).unwrap().project, None);

    let prompt = channel_prompt(&fixture.hub, projected.coordinator).unwrap();
    assert!(
        prompt.contains(&format!(
            "Project: {name} ({}). Agents you spawn work there.",
            folder.display()
        )),
        "{prompt}"
    );
    let prompt = channel_prompt(&fixture.hub, plain.coordinator).unwrap();
    assert!(
        prompt.contains("Project: none. Agents you spawn work in the channel's folder."),
        "{prompt}"
    );
}

#[tokio::test]
async fn updated_channels_show_their_new_title_and_archived_ones_leave_the_list() {
    let fixture = fixture("unused");
    let site = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let docs = create_channel(&fixture.hub, new_channel("docs"))
        .await
        .unwrap();

    update_channel(&fixture.hub, &site.id, "  ", "New plan.", None)
        .await
        .unwrap();
    let summary = summary_of(&channel_summaries(&fixture.hub).await.unwrap(), &site.id).unwrap();
    assert_eq!(summary.title, "Website relaunch", "a blank title is kept");
    assert_eq!(summary.description, "New plan.");

    archive_channel(&fixture.hub, &site.id).await.unwrap();
    let ids: Vec<String> = channel_summaries(&fixture.hub)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(ids, [docs.id]);
    let error = open_channel_record(&fixture.hub, &site.id)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("archived"), "{error:#}");
}

/// Keep a task of `agent` in `workspace`, made at `created_ms`.
fn write_task(workspace: &std::path::Path, id: &str, agent: AgentId, created_ms: u64) {
    use crate::event_loop::tasks::{TaskRecord, TASKS_DIR};
    let dir = workspace.join(TASKS_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    let record = TaskRecord {
        view: aura_ui::events::TaskView {
            id: id.to_string(),
            title: format!("task {id}"),
            agent_id: agent.to_hex(),
            agent_name: "someone".to_string(),
            updated_ms: created_ms,
            ..Default::default()
        },
        created_ms,
        tool_use_id: None,
    };
    std::fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn channel_tasks_gather_every_member_across_workspaces() {
    let mut fixture = fixture("Looks good.");
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let agent = spawn_under(&mut fixture, channel.coordinator).await;
    let elsewhere = tempfile::tempdir().unwrap();
    let (coordinator_folder, outsider) = {
        let mut registry = lock(&fixture.hub.registry);
        registry
            .set_folder(agent, elsewhere.path().to_path_buf())
            .unwrap();
        (
            registry.project_folder(channel.coordinator).unwrap(),
            registry.active().agent_id(),
        )
    };
    write_task(&coordinator_folder, "c-old", channel.coordinator, 1);
    write_task(&coordinator_folder, "o-mid", outsider, 2);
    write_task(elsewhere.path(), "a-new", agent, 3);
    fixture.drain_commands();

    crate::event_loop::tasks::send_channel(&fixture.hub, &channel.id)
        .await
        .unwrap();
    let tasks = fixture
        .take_commands()
        .into_iter()
        .find_map(|c| match c {
            UiCommand::SetChannelTasks { channel_id, tasks } if channel_id == channel.id => {
                Some(tasks)
            }
            _ => None,
        })
        .expect("the channel's tasks are sent");
    let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    // The spawn kept a task of its own too: the newest, so first.
    assert!(!ids.contains(&"o-mid"), "outsiders are left out: {ids:?}");
    assert_eq!(ids[ids.len() - 2..], ["a-new", "c-old"], "newest first");

    fixture.hub.factory.tasks.close_channel();
}

fn agent(byte: u8) -> AgentId {
    AgentId::new([byte; 32])
}

#[test]
fn member_mentions_match_codes_and_leave_other_words_alone() {
    use super::chat::parse_member_mentions;
    let (re, fx) = (agent(1), agent(2));
    let members = [
        (re, Some("RE36".to_string())),
        (fx, Some("FX2".to_string())),
        (agent(3), None),
    ];
    let cases: &[(&str, &[AgentId], bool)] = &[
        ("@re36, take a look", &[re], false),
        ("@FX2: and @RE36! then @fx2?", &[fx, re], false),
        ("@coordinator what now?", &[], true),
        ("@Coordinator. @RE36;", &[re], true),
        ("@foo and @src/main.rs and mail@RE36", &[], false),
        ("@ alone, @RE3 is not RE36", &[], false),
    ];
    for (text, mentioned, coordinator) in cases {
        assert_eq!(
            parse_member_mentions(text, &members),
            (mentioned.to_vec(), *coordinator),
            "{text}"
        );
    }
}

#[test]
fn the_digest_keeps_the_newest_lines_that_fit() {
    use super::chat::{render_digest, DIGEST_HEADER};
    assert_eq!(render_digest(&[]), None);
    let lines: Vec<String> = (0..100)
        .map(|i| format!("{i:03}: {}", "x".repeat(95)))
        .collect();
    let digest = render_digest(&lines).unwrap();
    assert!(digest.len() <= 4096, "{}", digest.len());
    assert!(digest.starts_with(DIGEST_HEADER));
    assert!(digest.ends_with(&lines[99]), "the newest line is kept");
    assert!(!digest.contains("000: "), "the oldest lines are dropped");
    let kept: Vec<&str> = digest.lines().skip(1).collect();
    let expected: Vec<&str> = lines[lines.len() - kept.len()..]
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(kept, expected, "oldest first");
}

/// How many listed agents the projects list counts under `project_id`;
/// `None` when the project is not on the list.
fn listed_count(fixture: &Fixture, project_id: &str) -> Option<usize> {
    let [UiCommand::SetProjects(projects), ..] = fixture.hub.agent_lists() else {
        panic!("the lists come as projects, agents, archive");
    };
    projects
        .iter()
        .find(|p| p.id == project_id)
        .map(|p| p.listed_agents)
}

#[tokio::test]
async fn switching_projects_moves_every_member_and_frees_the_old_project() {
    let mut fixture = fixture("Looks good.");
    let (dir_a, dir_b) = (
        tempfile::TempDir::new().unwrap(),
        tempfile::TempDir::new().unwrap(),
    );
    let (a, a_folder) = open_project(&fixture.hub, dir_a.path());
    let (b, _) = open_project(&fixture.hub, dir_b.path());
    let channel = create_channel(&fixture.hub, new_channel_in("site", &a))
        .await
        .unwrap();
    let agent = spawn_under(&mut fixture, channel.coordinator).await;
    // Each project has the agent it was opened with.
    assert_eq!(listed_count(&fixture, &a), Some(3));
    assert_eq!(listed_count(&fixture, &b), Some(1));

    update_channel(&fixture.hub, &channel.id, "", "", Some(b.clone()))
        .await
        .unwrap();
    assert_eq!(listed_count(&fixture, &a), Some(1));
    assert_eq!(listed_count(&fixture, &b), Some(3));
    for check in [placement, stored_placement] {
        assert_eq!(
            check(&fixture, agent),
            (Some(b.clone()), Some(a_folder.clone())),
            "in B, still working where it did"
        );
    }

    // What the member spawns now is in B too, working beside it.
    let later = spawn_under(&mut fixture, agent).await;
    assert_eq!(placement(&fixture, later).0, Some(b.clone()));
    assert_eq!(
        lock(&fixture.hub.registry).project_folder(later),
        Some(a_folder.clone())
    );
    assert_eq!(listed_count(&fixture, &b), Some(4));

    // A is free to remove, and removing it takes no member with it.
    assert_eq!(channel_in_project(&fixture.hub, &a).await.unwrap(), None);
    assert_eq!(
        channel_in_project(&fixture.hub, &b)
            .await
            .unwrap()
            .map(|c| c.id),
        Some(channel.id.clone())
    );
    let mut registry = lock(&fixture.hub.registry);
    let removed = registry.remove_project(&a).unwrap();
    for member in [channel.coordinator, agent, later] {
        assert!(!removed.contains(&member), "{}", member.to_hex());
        assert_eq!(
            registry.find(member).and_then(|m| m.project_id.clone()),
            Some(b.clone())
        );
    }
}

#[tokio::test]
async fn a_project_a_member_is_in_counts_as_used_by_its_channel() {
    let mut fixture = fixture("Looks good.");
    let (dir_a, dir_b) = (
        tempfile::TempDir::new().unwrap(),
        tempfile::TempDir::new().unwrap(),
    );
    let (a, _) = open_project(&fixture.hub, dir_a.path());
    let (b, _) = open_project(&fixture.hub, dir_b.path());
    let channel = create_channel(&fixture.hub, new_channel_in("site", &a))
        .await
        .unwrap();
    let agent = spawn_under(&mut fixture, channel.coordinator).await;
    // A member left in B, as a change that failed half-way could leave it.
    lock(&fixture.hub.registry)
        .place_agents(
            &[crate::agent_registry::AgentPlacement {
                agent_id: agent,
                project_id: Some(b.clone()),
                folder: None,
            }],
            |agents, rows| {
                agents.apply(&aura_store_db::RegistryDiff {
                    agents_put: rows,
                    ..Default::default()
                })?;
                Ok(())
            },
        )
        .unwrap();
    // No channel names B, but a member of a channel with a project is in it.
    let used = channel_in_project(&fixture.hub, &b).await.unwrap();
    assert_eq!(used.map(|c| c.id), Some(channel.id.clone()));

    archive_channel(&fixture.hub, &channel.id).await.unwrap();
    assert_eq!(channel_in_project(&fixture.hub, &b).await.unwrap(), None);
}

#[tokio::test]
async fn members_of_a_channel_without_a_project_dont_keep_the_launch_project() {
    let mut fixture = fixture("Looks good.");
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let agent = spawn_under(&mut fixture, channel.coordinator).await;
    let launch = placement(&fixture, agent).0.unwrap();
    // Its members sit, hidden, in the launch project; removing that
    // project keeps them, so it is not the channel's to hold on to.
    assert_eq!(
        channel_in_project(&fixture.hub, &launch).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn clearing_a_project_keeps_it_on_the_projects_list() {
    let mut fixture = fixture("Looks good.");
    let outsider = lock(&fixture.hub.registry).active().agent_id();
    let launch = placement(&fixture, outsider).0.unwrap();
    let channel = create_channel(&fixture.hub, new_channel_in("site", &launch))
        .await
        .unwrap();
    let agent = spawn_under(&mut fixture, channel.coordinator).await;
    // Only the channel's agents are left in it, so once they are hidden
    // nothing listed is in it.
    {
        let mut registry = lock(&fixture.hub.registry);
        let others: Vec<AgentId> = registry
            .tree()
            .into_iter()
            .map(|(a, _)| a.agent_id())
            .filter(|id| ![channel.coordinator, agent].contains(id))
            .collect();
        for other in others {
            registry.archive(other).unwrap();
        }
    }
    assert_eq!(listed_count(&fixture, &launch), Some(2));

    update_channel(&fixture.hub, &channel.id, "", "", None)
        .await
        .unwrap();
    let (rows, projects) = projects_tree(&fixture);
    assert!(projects.contains(&launch), "{projects:?}");
    assert!(!is_listed(&rows, channel.coordinator));
    assert!(!is_listed(&rows, agent));
    assert_eq!(listed_count(&fixture, &launch), Some(0));
}

#[tokio::test]
async fn archived_channels_cannot_be_changed() {
    let fixture = fixture("unused");
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    archive_channel(&fixture.hub, &channel.id).await.unwrap();
    let error = update_channel(&fixture.hub, &channel.id, "New", "", None)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("archived"), "{error:#}");
    assert_eq!(
        store(&fixture).get(&channel.id).unwrap().unwrap().title,
        channel.title
    );
}
