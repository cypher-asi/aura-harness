//! Creating channels, their coordinators and what coordinators may do.

use super::super::*;
use super::support::{add_member, agent_count, new_channel, spawn_under, store};
use crate::event_loop::subagents::tests::{fixture, Fixture};
use crate::event_loop::subagents::{assign_tool_definition, task_tool_definition};

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
    assert_eq!(coordinator.name, "coordinator");
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

/// The code of `agent`.
fn code_of(fixture: &Fixture, agent: AgentId) -> String {
    lock(&fixture.hub.registry)
        .find(agent)
        .and_then(|a| a.code.clone())
        .unwrap()
}

#[tokio::test]
async fn channel_prompt_gives_the_coordinator_the_channel_and_how_it_works() {
    let fixture = fixture("unused");
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let code = code_of(&fixture, channel.coordinator);
    let member = add_member(&fixture, channel.coordinator, "reviewer");

    let prompt = channel_prompt(&fixture.hub, channel.coordinator).unwrap();
    assert!(prompt.starts_with("<channel>\nYou coordinate the channel #site: Website relaunch\n"));
    assert!(prompt.contains("Ship the new marketing site."), "{prompt}");
    assert!(
        prompt.contains(&format!("- {code} coordinator (you)\n")),
        "{prompt}"
    );
    assert!(
        prompt.contains(&format!("- {} reviewer", code_of(&fixture, member))),
        "{prompt}"
    );
    assert!(prompt.contains(": free\n"), "{prompt}");
    for rule in [
        "You don't do the work",
        "with `assign`",
        "never how to do it",
        "subagents included, is its own call",
        "at most three short lines",
    ] {
        assert!(prompt.contains(rule), "{rule}: {prompt}");
    }
    assert!(prompt.ends_with("</channel>"));

    let outsider = lock(&fixture.hub.registry).active().agent_id();
    assert_eq!(channel_prompt(&fixture.hub, outsider), None);
}

#[tokio::test]
async fn channel_prompt_tells_a_member_how_members_work_together() {
    let mut fixture = fixture("Looks good.");
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let lead = code_of(&fixture, channel.coordinator);
    let member = add_member(&fixture, channel.coordinator, "reviewer");
    let other = add_member(&fixture, channel.coordinator, "tests");
    let subagent = spawn_under(&mut fixture, member).await;

    let prompt = channel_prompt(&fixture.hub, member).unwrap();
    assert!(
        prompt.starts_with(&format!(
            "<channel>\nYou are {} reviewer #",
            code_of(&fixture, member)
        )),
        "{prompt}"
    );
    assert!(
        prompt.contains(&format!(
            "a member of the channel #site: Website relaunch, which {lead} coordinates."
        )),
        "{prompt}"
    );
    assert!(
        prompt.contains(&format!(
            "Other members: {} tests",
            code_of(&fixture, other)
        )),
        "{prompt}"
    );
    assert!(prompt.contains("whether to hand parts of it to subagents with `task`"));
    assert!(prompt.contains("with `send_to_agent`"));
    assert!(prompt.contains(&format!("Keep {lead} to results, blockers and decisions.")));
    assert_eq!(
        channel_prompt(&fixture.hub, subagent),
        None,
        "a subagent hears from the agent that spawned it"
    );
}

#[test]
fn the_coordinator_is_listed_but_never_spawned_or_assigned() {
    let fixture = fixture("unused");
    let definitions = lock(&fixture.hub.definitions);
    assert!(definitions.get(COORDINATOR_DEFINITION).is_some());
    let task = task_tool_definition(&definitions).unwrap();
    let assign = assign_tool_definition(&definitions).unwrap();
    for names in [
        &task.input_schema["properties"]["subagent_type"]["enum"],
        &assign.input_schema["properties"]["agent"]["enum"],
    ] {
        let names = names.as_array().unwrap();
        assert!(names.iter().any(|name| name == "reviewer"), "{names:?}");
        assert!(
            names.iter().all(|name| name != COORDINATOR_DEFINITION),
            "{names:?}"
        );
    }
    drop(definitions);
    assert!(
        fixture
            .hub
            .definition_summaries()
            .iter()
            .any(|summary| summary.name == COORDINATOR_DEFINITION),
        "the AGENTS tab shows the coordinator, to edit it"
    );
}

#[test]
fn coordinator_turns_only_assign_and_check_in() {
    let fixture = fixture("unused");
    let workspace = lock(&fixture.hub.registry).data_dir().to_path_buf();
    let (_, tools) = fixture.hub.turn(Some(COORDINATOR_DEFINITION), &workspace);
    let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "assign",
            "get_agent_state",
            "list_agents",
            "send_to_agent",
            "todo_write"
        ]
    );
}

#[tokio::test]
async fn members_always_keep_task_and_their_inbox_but_subagents_get_what_they_list() {
    let mut fixture = fixture("Looks good.");
    let channel = create_channel(&fixture.hub, new_channel("site"))
        .await
        .unwrap();
    let member = add_member(&fixture, channel.coordinator, "reviewer");
    let subagent = spawn_under(&mut fixture, member).await;
    let workspace = lock(&fixture.hub.registry).data_dir().to_path_buf();
    let names = |agent: AgentId| -> Vec<String> {
        let (_, tools) = fixture.hub.agent_turn(agent, Some("reviewer"), &workspace);
        tools.into_iter().map(|t| t.name).collect()
    };

    let top = names(member);
    for tool in [
        "task",
        "send_to_agent",
        "list_agents",
        "get_agent_state",
        "read_file",
    ] {
        assert!(top.iter().any(|name| name == tool), "{tool}: {top:?}");
    }
    assert!(top
        .iter()
        .all(|name| name != "write_file" && name != "assign"));

    let below = names(subagent);
    assert!(below.iter().any(|name| name == "read_file"), "{below:?}");
    for tool in ["task", "send_to_agent", "list_agents", "assign"] {
        assert!(below.iter().all(|name| name != tool), "{tool}: {below:?}");
    }
}
