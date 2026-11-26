//! Creating channels, their coordinators and what coordinators may do.

use super::super::*;
use super::support::{agent_count, new_channel, store};
use crate::event_loop::subagents::task_tool_definition;
use crate::event_loop::subagents::tests::fixture;

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
