use super::*;
use crate::event_loop::subagents::task_tool_definition;
use crate::event_loop::subagents::tests::{fixture, request, Fixture};
use aura_ui_terminal::UiCommand;

fn new_channel(slug: &str) -> NewChannel {
    NewChannel {
        slug: slug.to_string(),
        title: "Website relaunch".to_string(),
        description: "Ship the new marketing site.".to_string(),
    }
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
}
