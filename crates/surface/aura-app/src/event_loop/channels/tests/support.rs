//! Helpers shared by the channel tests.

use super::super::*;
use crate::event_loop::subagents::tests::{request, Fixture};
use aura_ui::UiCommand;

pub(super) fn new_channel(slug: &str) -> NewChannel {
    NewChannel {
        slug: slug.to_string(),
        title: "Website relaunch".to_string(),
        description: "Ship the new marketing site.".to_string(),
        ..NewChannel::default()
    }
}

/// [`new_channel`] working in project `project_id`.
pub(super) fn new_channel_in(slug: &str, project_id: &str) -> NewChannel {
    NewChannel {
        project_id: Some(project_id.to_string()),
        ..new_channel(slug)
    }
}

/// What the edit form sends to give a local channel `title`,
/// `description` and `project_id`.
pub(super) fn edit(title: &str, description: &str, project_id: Option<String>) -> ChannelEdit {
    ChannelEdit {
        title: title.to_string(),
        description: description.to_string(),
        project_id,
        placement: ChannelPlacement::Local,
    }
}

/// The project and own folder `agent` has in the registry.
pub(super) fn placement(fixture: &Fixture, agent: AgentId) -> (Option<String>, Option<PathBuf>) {
    let registry = lock(&fixture.hub.registry);
    let found = registry.find(agent).unwrap();
    (found.project_id.clone(), found.folder.clone())
}

/// The stored row of `agent`: its project and own folder.
pub(super) fn stored_placement(
    fixture: &Fixture,
    agent: AgentId,
) -> (Option<String>, Option<PathBuf>) {
    let rows = lock(&fixture.hub.registry).store().load().unwrap().agents;
    let row = rows.iter().find(|row| row.agent_id == agent).unwrap();
    (row.project_id.clone(), row.folder.clone())
}

/// The ids in the Agents panel rows and the ids of the projects listed.
pub(super) fn projects_tree(
    fixture: &Fixture,
) -> (Vec<aura_ui::events::AgentSummary>, Vec<String>) {
    let [UiCommand::SetProjects(projects), UiCommand::SetAgents(rows), _] =
        fixture.hub.agent_lists()
    else {
        panic!("the lists come as projects, agents, archive");
    };
    (rows, projects.into_iter().map(|p| p.id).collect())
}

pub(super) fn is_listed(rows: &[aura_ui::events::AgentSummary], agent: AgentId) -> bool {
    rows.iter().any(|row| row.id == agent.to_hex())
}

/// Clear `agent`'s own folder, in memory and stored, keeping it in
/// `project_id`, as a member that joined before folders were pinned.
pub(super) fn unpin(fixture: &Fixture, agent: AgentId, project_id: Option<String>) {
    let unpinned = AgentPlacement {
        agent_id: agent,
        project_id,
        folder: None,
    };
    lock(&fixture.hub.registry)
        .place_agents(&[unpinned], |agents, rows| -> anyhow::Result<()> {
            let mut batch = WriteBatch::default();
            stage_rows(agents, rows, &mut batch)?;
            Ok(store(fixture).write(batch)?)
        })
        .unwrap();
    assert_eq!(stored_placement(fixture, agent).1, None);
}

/// Start a new member of the channel `coordinator` runs, from the
/// permanent agent `definition`, as `assign` does.
pub(super) fn add_member(fixture: &Fixture, coordinator: AgentId, definition: &str) -> AgentId {
    add_agent(&fixture.hub, coordinator, definition)
        .unwrap()
        .agent_id()
}

pub(super) fn agent_count(fixture: &Fixture) -> usize {
    lock(&fixture.hub.registry).agents().len()
}

pub(super) fn store(fixture: &Fixture) -> &ChannelStore {
    fixture.hub.channels.as_ref().unwrap()
}

/// Spawn a reviewer under `parent` through the real `task` dispatch path;
/// returns the child. Drains the UI commands the spawn sent: each one
/// sends a few dozen and nothing else reads the fixture's 64-slot
/// channel, so a third undrained spawn would wait on it forever.
pub(super) async fn spawn_under(fixture: &mut Fixture, parent: AgentId) -> AgentId {
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

pub(super) fn summary_of(summaries: &[ChannelSummary], id: &str) -> Option<ChannelSummary> {
    summaries.iter().find(|s| s.id == id).cloned()
}

/// Keep a task of `agent` in `workspace`, made at `created_ms`.
pub(super) fn write_task(workspace: &std::path::Path, id: &str, agent: AgentId, created_ms: u64) {
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

pub(super) fn agent(byte: u8) -> AgentId {
    AgentId::new([byte; 32])
}

/// How many listed agents the projects list counts under `project_id`;
/// `None` when the project is not on the list.
pub(super) fn listed_count(fixture: &Fixture, project_id: &str) -> Option<usize> {
    let [UiCommand::SetProjects(projects), ..] = fixture.hub.agent_lists() else {
        panic!("the lists come as projects, agents, archive");
    };
    projects
        .iter()
        .find(|p| p.id == project_id)
        .map(|p| p.listed_agents)
}
