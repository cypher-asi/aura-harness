use super::*;

/// `UiEvent::CreateChannel`: make the channel, list it and open it. The
/// agent list goes out too, as a coordinator in a project is listed
/// under it.
pub(in crate::event_loop) async fn handle_create(state: &mut LoopState<'_>, new: NewChannel) {
    match create_channel(&state.hub, new).await {
        Ok(channel) => {
            state.hub.send_agent_list().await;
            handle_open(state, &channel.id).await;
        }
        Err(error) => show_error(state, format!("Could not create the channel: {error:#}")).await,
    }
}

/// `UiEvent::UpdateChannel`: change the channel, then send the agent list
/// (and the channels with it), as a project change moves its members in
/// or out of the Projects tree. A project change is refused while the
/// coordinator runs a turn; once it is made, the coordinator's session,
/// which works in the old folder, is dropped so its next turn opens in
/// the new one, and the open channel's tasks are read again from where
/// its members work now.
pub(in crate::event_loop) async fn handle_update(
    state: &mut LoopState<'_>,
    channel_id: &str,
    edit: ChannelEdit,
) {
    let updated = match update_channel(&state.hub, channel_id, edit).await {
        Ok(updated) => updated,
        Err(error) => {
            let message = match error.downcast_ref::<CoordinatorBusy>() {
                Some(busy) => busy.to_string(),
                None => format!("Could not update the channel: {error:#}"),
            };
            show_error(state, message).await;
            return;
        }
    };
    if updated.moved {
        reopen_coordinator(state, updated.coordinator).await;
        if state.open_channel.as_deref() == Some(channel_id) {
            if let Err(error) = crate::event_loop::tasks::send_channel(&state.hub, channel_id).await
            {
                warn!(
                    channel_id,
                    error = %format!("{error:#}"),
                    "Could not show the channel's tasks after its project changed"
                );
            }
        }
    }
    state.hub.send_agent_list().await;
}

/// Drop the idle session of `coordinator`, opened in the folder it had
/// before its channel's project changed, so its next turn opens a session
/// in the new one. When it is the active agent its new session opens now
/// and the Sandbox panel follows it.
pub(super) async fn reopen_coordinator(state: &mut LoopState<'_>, coordinator: AgentId) {
    if state.idle.remove(&coordinator).is_none() || state.active != coordinator {
        return;
    }
    // On failure `open_session` logged why; the next turn opens it lazily.
    if let Ok(session) = crate::event_loop::agents::open_session(state, coordinator) {
        state.idle.insert(coordinator, session);
        crate::event_loop::agents::send_sandbox_root(state).await;
    }
}

/// `UiEvent::OpenChannel`: make the channel's coordinator the active
/// agent, as selecting it would, and show the channel's tasks. A UI that
/// put the coordinator on screen itself asked for it just before
/// (`SelectAgent` or `FocusAgent`), so it is active already and its chat
/// is not loaded a second time.
pub(in crate::event_loop) async fn handle_open(state: &mut LoopState<'_>, channel_id: &str) {
    crate::event_loop::factory::service::show_channel(channel_id);
    let channel = match open_channel_record(&state.hub, channel_id).await {
        Ok(channel) => channel,
        Err(error) => {
            show_error(state, format!("Could not open the channel: {error:#}")).await;
            return;
        }
    };
    if state.active != channel.coordinator {
        crate::event_loop::agents::handle_select_agent(state, &channel.coordinator.to_hex()).await;
    }
    state.open_channel = Some(channel.id.clone());
    if let Err(error) = crate::event_loop::tasks::send_channel(&state.hub, &channel.id).await {
        warn!(
            channel_id = %channel.id,
            error = %format!("{error:#}"),
            "Could not show the channel's tasks"
        );
    }
}

/// `UiEvent::CloseChannel`: no channel is open any more.
pub(in crate::event_loop) fn handle_close(state: &mut LoopState<'_>) {
    state.open_channel = None;
    state.hub.factory.tasks.close_channel();
}

/// `UiEvent::ArchiveChannel`: archive it, closing it when it is open.
pub(in crate::event_loop) async fn handle_archive(state: &mut LoopState<'_>, channel_id: &str) {
    let archived = archive_channel(&state.hub, channel_id).await;
    if archived.is_ok() && state.open_channel.as_deref() == Some(channel_id) {
        handle_close(state);
    }
    after_change(state, "archive the channel", archived).await;
}

/// After a channel action: list the channels again when it worked, else
/// tell the user it could not `what`.
async fn after_change(state: &LoopState<'_>, what: &str, result: anyhow::Result<()>) {
    match result {
        Ok(()) => send_channels(&state.hub).await,
        Err(error) => show_error(state, format!("Could not {what}: {error:#}")).await,
    }
}

pub(super) async fn show_error(state: &LoopState<'_>, message: String) {
    let _ = state.commands.send(UiCommand::ShowError(message)).await;
}
