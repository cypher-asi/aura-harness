//! Who joins a channel, how members are listed and what they keep.

use super::super::*;
use super::support::{
    add_member, agent_count, new_channel, placement, spawn_under, store, write_task,
};
use crate::event_loop::subagents::tests::fixture;
use aura_ui::UiCommand;

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
async fn start_up_makes_the_agents_a_coordinator_spawned_its_channels_own() {
    let mut fixture = fixture("Looks good.");
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    // As coordinators used to: spawned under the coordinator itself.
    let agent = spawn_under(&mut fixture, channel.coordinator).await;
    let subagent = spawn_under(&mut fixture, agent).await;
    let (project, folder) = placement(&fixture, agent);
    assert!(!lock(&fixture.hub.registry).is_top_level_agent(agent));

    for _ in 0..2 {
        load_channels(Some(store(&fixture)), &mut lock(&fixture.hub.registry));
    }

    let registry = lock(&fixture.hub.registry);
    let promoted = registry.find(agent).unwrap();
    assert_eq!(promoted.parent, None);
    assert!(registry.is_top_level_agent(agent));
    assert_eq!(
        (promoted.project_id.clone(), promoted.folder.clone()),
        (project, folder)
    );
    assert_eq!(
        registry.find(subagent).unwrap().parent.as_deref(),
        Some(promoted.zns_id.as_str()),
        "its own subagents stay under it"
    );
    let rows = registry.store().load().unwrap().agents;
    let row = rows.iter().find(|row| row.agent_id == agent).unwrap();
    assert_eq!(row.parent, None, "stored, so a restart keeps it");
    drop(registry);

    let members: Vec<_> = store(&fixture)
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
}

#[tokio::test]
async fn a_new_member_lands_with_its_member_row_and_only_its_coordinator_adds_one() {
    let fixture = fixture("unused");
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let member = add_member(&fixture, channel.coordinator, "reviewer");

    let rows = lock(&fixture.hub.registry).store().load().unwrap().agents;
    assert!(rows
        .iter()
        .any(|row| row.agent_id == member && row.parent.is_none()));
    assert_eq!(
        store(&fixture).channel_of(&member).unwrap().as_deref(),
        Some(channel.id.as_str())
    );
    assert!(lock(&fixture.hub.channel_members).contains(&member));
    assert!(lock(&fixture.hub.hidden_members).contains(&member));

    let before = agent_count(&fixture);
    let outsider = lock(&fixture.hub.registry).active().agent_id();
    for caller in [member, outsider] {
        assert!(add_agent(&fixture.hub, caller, "reviewer").is_err());
    }
    assert_eq!(agent_count(&fixture), before);
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
