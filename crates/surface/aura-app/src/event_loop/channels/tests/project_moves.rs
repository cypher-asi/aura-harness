//! Moving a channel between projects and which projects it holds on to.

use super::super::*;
use super::support::{
    is_listed, listed_count, new_channel, new_channel_in, placement, projects_tree, spawn_under,
    stored_placement,
};
use crate::event_loop::subagents::tests::fixture;
use crate::event_loop::tests::support::open_project;

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
            |agents, rows| -> anyhow::Result<()> {
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
