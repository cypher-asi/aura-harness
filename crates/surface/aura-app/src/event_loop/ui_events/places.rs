//! UI events for channels, where agents run and cloud machines' files.

use crate::event_loop::{channels, cloud, LoopState};
use aura_ui::UiEvent;

/// Channels and cloud placement.
pub(super) async fn handle(state: &mut LoopState<'_>, event: UiEvent) {
    match event {
        UiEvent::SetPlacement {
            agent_id,
            placement,
        } => cloud::handle_set_placement(state, &agent_id, placement).await,
        UiEvent::HostAction { agent_id, action } => {
            cloud::handle_host_action(state, &agent_id, action).await;
        }
        UiEvent::CloudSetup => cloud::handle_setup(state).await,
        UiEvent::ListSandbox(dir) => cloud::list_sandbox(state, dir),
        UiEvent::ReadSandboxFile(path) => cloud::read_sandbox_file(state, path),
        UiEvent::LoadMergeHistory { channel_id, before } => {
            crate::event_loop::factory::service::load_history(&channel_id, before);
        }
        channel_event => channel(state, channel_event).await,
    }
}

/// Making, opening, changing and archiving channels, and `/plan` in a
/// channel's chat.
async fn channel(state: &mut LoopState<'_>, event: UiEvent) {
    match event {
        UiEvent::CreateChannel {
            slug,
            title,
            description,
            project_id,
            placement,
        } => {
            let new = aura_store_db::NewChannel {
                slug,
                title,
                description,
                project_id,
                placement: channels::channel_placement(placement),
            };
            channels::handle_create(state, new).await;
        }
        UiEvent::OpenChannel(channel_id) => channels::handle_open(state, &channel_id).await,
        UiEvent::CloseChannel => channels::handle_close(state),
        UiEvent::UpdateChannel {
            channel_id,
            title,
            description,
            project_id,
            placement,
        } => {
            let edit = channels::ChannelEdit {
                title,
                description,
                project_id,
                placement: channels::channel_placement(placement),
            };
            channels::handle_update(state, &channel_id, edit).await;
        }
        UiEvent::ArchiveChannel(channel_id) => channels::handle_archive(state, &channel_id).await,
        UiEvent::PlanInChannel {
            agent_id,
            text,
            images,
        } => {
            let coordinator = state.agent_or_active(&agent_id);
            channels::start_planner(state, coordinator, text, images).await;
        }
        // `domain_of` routes no other event here.
        _ => {}
    }
}
