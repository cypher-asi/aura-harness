//! What the hub does with stored channels as it starts up.

use super::super::*;
use super::support::{
    is_listed, new_channel, new_channel_in, placement, projects_tree, spawn_under, store,
    stored_placement, summary_of, unpin,
};
use crate::event_loop::subagents::tests::fixture;
use crate::event_loop::tests::support::open_project;

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
