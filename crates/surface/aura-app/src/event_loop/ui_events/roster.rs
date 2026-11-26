//! UI events that manage agents, projects and definitions.

use crate::event_loop::{agents, LoopState};
use aura_ui::UiEvent;

/// Agents, projects and definitions.
pub(super) async fn handle(state: &mut LoopState<'_>, event: UiEvent) {
    match event {
        UiEvent::SelectAgent(agent_hex) => agents::handle_select_agent(state, &agent_hex).await,
        UiEvent::FocusAgent(agent_hex) => agents::handle_focus_agent(state, &agent_hex).await,
        UiEvent::CreateAgent(name) => agents::handle_create_agent(state, name).await,
        UiEvent::CreateProject { folder } => agents::handle_create_project(state, &folder).await,
        UiEvent::AddAgentToProject(project) => {
            agents::handle_add_agent_to_project(state, &project).await;
        }
        UiEvent::ReorderProjects(order) => agents::handle_reorder_projects(state, &order).await,
        UiEvent::ReorderAgents { project_id, order } => {
            agents::handle_reorder_agents(state, &project_id, &order).await;
        }
        UiEvent::ArchiveAgent(agent) => agents::handle_archive_agent(state, &agent).await,
        UiEvent::RestoreAgent(agent) => agents::handle_restore_agent(state, &agent).await,
        UiEvent::RemoveProject(project) => agents::handle_remove_project(state, &project).await,
        UiEvent::RefreshAgents => agents::handle_refresh_agents(state).await,
        UiEvent::InstantiateDefinition(name) => {
            agents::handle_instantiate_definition(state, &name).await;
        }
        UiEvent::AddDefinitionToProject(name) => {
            agents::handle_add_definition_to_project(state, &name).await;
        }
        UiEvent::EditDefinition(name) => agents::handle_edit_definition(state, name).await,
        UiEvent::DeleteDefinition(name) => agents::handle_delete_definition(state, &name).await,
        UiEvent::DefinitionEdited(path) => agents::handle_definition_edited(state, &path).await,
        // `domain_of` routes no other event here.
        _ => {}
    }
}
