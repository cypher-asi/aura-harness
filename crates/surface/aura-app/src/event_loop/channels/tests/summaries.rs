//! Channel summaries, prompts naming the project, updates and archiving.

use super::super::*;
use super::support::{new_channel, new_channel_in, store, summary_of};
use crate::event_loop::subagents::tests::{fixture, request};
use crate::event_loop::tests::support::open_project;
use aura_ui::UiCommand;

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
