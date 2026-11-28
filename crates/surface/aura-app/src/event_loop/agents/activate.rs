//! Opening sessions and switching the UI to an agent.

use super::*;

pub(crate) async fn handle_refresh_agents(state: &LoopState<'_>) {
    state.hub.send_agent_list().await;
    state.hub.send_definitions().await;
}

/// Open a session for `agent_id` from its record.
pub(crate) fn open_session(
    state: &LoopState<'_>,
    agent_id: AgentId,
) -> Result<AgentSession, String> {
    open_registered(&state.hub, agent_id, |registered, folder| {
        state.hub.factory.open(registered, Vec::new(), folder)
    })
}

/// [`open_session`] from `tail`, the newest entries of the agent's
/// record, read already.
pub(super) fn open_session_from(
    hub: &AgentHub,
    agent_id: AgentId,
    tail: &RecordTail,
) -> Result<AgentSession, String> {
    open_registered(hub, agent_id, |registered, folder| {
        hub.factory.open_from(registered, Vec::new(), folder, tail)
    })
}

/// Open the registered agent `agent_id` in its project's folder with
/// `open`.
pub(super) fn open_registered(
    hub: &AgentHub,
    agent_id: AgentId,
    open: impl FnOnce(&RegisteredAgent, Option<PathBuf>) -> anyhow::Result<AgentSession>,
) -> Result<AgentSession, String> {
    let (registered, folder) = {
        let registry = lock(&hub.registry);
        (
            registry.find(agent_id).cloned(),
            registry.project_folder(agent_id),
        )
    };
    let Some(registered) = registered else {
        return Err("Unknown agent".to_string());
    };
    open(&registered, folder).map_err(|e| {
        warn!(error = %e, "Failed to open agent kernel");
        format!("Could not open agent {}: {e}", registered.name)
    })
}

/// What a switch reads of the agent coming on screen.
pub(super) struct SwitchReads {
    /// The session opened for it, when it had none
    pub(super) session: Option<Result<AgentSession, String>>,
    /// Its chat, when one is to be sent
    pub(super) chat: Option<Vec<MessageData>>,
}

/// Read what a switch to `agent_id` needs, on a blocking thread: with
/// `open` a session opened from its record, and with `chat` its chat.
/// Both come from one read of the record's newest entries, and the chat
/// is rebuilt while the session opens. With `activate` the agent, once
/// open, is saved as the active one.
pub(super) fn read_for_switch(
    hub: &AgentHub,
    agent_id: AgentId,
    open: bool,
    chat: bool,
    activate: bool,
) -> SwitchReads {
    let store = hub.factory.store.as_ref();
    let kept = if chat {
        let head = store.get_head_seq(agent_id).ok();
        head.and_then(|head| lock(&hub.shown_chats).kept(agent_id, head))
    } else {
        None
    };
    let rebuild = chat && kept.is_none();
    let tail = if open || rebuild {
        RecordTail::read(store, agent_id)
            .inspect_err(|e| warn!(%agent_id, error = %e, "Failed to read the record"))
            .ok()
    } else {
        None
    };
    // An unreadable record opens the agent with an empty conversation and
    // shows an empty chat.
    let unread = RecordTail::default();
    let entries = tail.as_ref().unwrap_or(&unread);
    let (session, rebuilt) = std::thread::scope(|scope| {
        let rebuilt = rebuild.then(|| scope.spawn(|| conversation_from(entries.entries())));
        // Here, not on the scoped thread: building a kernel needs the
        // runtime this blocking thread belongs to.
        let session = open.then(|| open_session_from(hub, agent_id, entries));
        let rebuilt =
            rebuilt.map(|job| job.join().unwrap_or_else(|e| std::panic::resume_unwind(e)));
        (session, rebuilt)
    });
    if let (Some(chat), Some(tail)) = (&rebuilt, &tail) {
        lock(&hub.shown_chats).keep(agent_id, tail.head(), chat.clone());
    }
    if activate && session.as_ref().is_none_or(Result::is_ok) {
        if let Err(e) = lock(&hub.registry).set_active(agent_id) {
            warn!(error = %e, "Failed to persist active agent");
        }
    }
    SwitchReads {
        session,
        chat: kept
            .or(rebuilt)
            .map(|chat| crate::chat_notes::with_notes(agent_id, chat)),
    }
}

/// The chat of `agent_id` as its record has it now, when the record grew
/// past the chat last read of it; `None` when it didn't. Reads the
/// record: callers on the runtime run it on a blocking thread.
pub(crate) fn newer_chat(hub: &AgentHub, agent_id: AgentId) -> Option<Vec<MessageData>> {
    let head = hub.factory.store.get_head_seq(agent_id).ok()?;
    if lock(&hub.shown_chats).kept(agent_id, head).is_some() {
        return None;
    }
    read_for_switch(hub, agent_id, false, true, false).chat
}

/// Confirm the agent on screen to a UI that asked to switch to it, with
/// its lists.
async fn confirm_active(state: &LoopState<'_>, agent_id: AgentId) {
    // Confirm it all the same: a UI that switched on screen ahead
    // waits for this answer before taking the session's next one.
    let _ = state
        .commands
        .send(UiCommand::SetActiveAgent(agent_id.to_hex()))
        .await;
    if let Some(lists) = state.hub.lists_for_switch(agent_id).await {
        state.hub.send_switch_lists(lists).await;
    }
}

/// Keep the session a switch opened, if it opened one; `false` when it
/// could not open the agent, which the UI is then told.
async fn keep_session(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    session: Option<Result<AgentSession, String>>,
) -> bool {
    match session {
        Some(Ok(session)) => {
            state.idle.insert(agent_id, session);
        }
        Some(Err(problem)) => {
            show_error(state, problem).await;
            let _ = state
                .commands
                .send(UiCommand::SwitchFailed(agent_id.to_hex()))
                .await;
            return false;
        }
        None => {}
    }
    true
}

/// Show `agent_id`. Agents keep working while another one is shown.
///
/// A UI switches on screen before it asks, so every select gets an
/// answer it can end its wait on: the agent's chat, [`UiCommand::ChatReady`]
/// for a live one, or [`UiCommand::SwitchFailed`].
pub(super) async fn activate_agent(state: &mut LoopState<'_>, agent_id: AgentId, chat: Chat) {
    state.park_finished_spawns().await;
    let switching = agent_id != state.active;
    if !switching && chat == Chat::Keep {
        confirm_active(state, agent_id).await;
        return;
    }
    if switching {
        // What it would still send is for the agent going off screen.
        state.stop_side_panels();
    }
    // A UI that showed another agent in the meantime asks for the active
    // one's chat again; it gets it as any other switch does.
    let started = Instant::now();
    let running = state.hub.is_running(agent_id);
    let open = !running && !state.idle.contains_key(&agent_id);
    // A cloud turn's chat is live in the UI as a local turn's is.
    let load_chat = chat == Chat::Reload && !running && !state.cloud.is_running(agent_id);
    let hub = Arc::clone(&state.hub);
    let (read, lists) = tokio::join!(
        tokio::task::spawn_blocking(move || {
            read_for_switch(&hub, agent_id, open, load_chat, switching)
        }),
        state.hub.lists_for_switch(agent_id),
    );
    let read = read.unwrap_or_else(|e| SwitchReads {
        session: Some(Err(format!("Could not open the agent: {e}"))),
        chat: None,
    });
    let read_ms = ms_since(started);
    if !keep_session(state, agent_id, read.session).await {
        return;
    }
    state.active = agent_id;
    show_active_agent(state, chat, read.chat, lists).await;
    debug!(
        %agent_id,
        opened = open,
        read_ms,
        total_ms = ms_since(started),
        "Switched to an agent"
    );
}

/// Switch the UI to the active agent: its lists, with [`Chat::Reload`] its
/// chat (`history`, or [`UiCommand::ChatReady`] for a busy agent, whose
/// chat is live in the UI already), then its panels. The panels beside
/// the chat fill in after the switch, in a job of their own.
pub(super) async fn show_active_agent(
    state: &mut LoopState<'_>,
    chat: Chat,
    history: Option<Vec<MessageData>>,
    lists: Option<AgentLists>,
) {
    let agent_id = state.active;
    // A cloud turn streams into the chat the way a local one does.
    let running = state.hub.is_running(agent_id) || state.cloud.is_running(agent_id);

    state.hub.record_panel.set(agent_id);
    let _ = state
        .commands
        .send(UiCommand::SetActiveAgent(agent_id.to_hex()))
        .await;
    // The lists go first: a new or restored agent shows up in them, and an
    // archived one leaves them, before its history is sent.
    if let Some(lists) = lists {
        state.hub.send_switch_lists(lists).await;
    }
    // The chat next: it is what the UI waits on, blank.
    if chat == Chat::Reload {
        // A coordinator's chat shows its channel's posts among its
        // messages. They go first, so the chat shows with them instead of
        // reflowing a moment later when they come.
        let mine =
            super::super::channels::send_posts_of(state.commands, &state.hub, agent_id).await;
        if let Some((channel_id, mut posts)) = mine {
            // HISTORY lists what the user typed to the coordinator too.
            use super::super::channels::summaries;
            let typed = history.as_deref().map(summaries::prompts_of);
            posts.extend(typed.unwrap_or_default());
            summaries::spawn_for(state, agent_id, &channel_id, posts);
        }
        let ui = AgentUi::new(state.commands.clone(), agent_id);
        match history {
            Some(history) => {
                ui.send(UiCommand::LoadConversation(history)).await;
                super::super::hud::send_session_of(state, agent_id).await;
            }
            // Say so, so a UI that switched to it ahead of this answer
            // stops waiting.
            None => ui.send(UiCommand::ChatReady).await,
        }
    }

    super::super::modes::send_mode(state, agent_id).await;
    super::super::policies::send_policies_of(state, agent_id).await;
    super::super::hud::send_instructions_of(state, agent_id).await;
    send_sandbox_root(state).await;
    if running {
        // Its turn shows its plan again when it next changes.
        let _ = state.commands.send(UiCommand::SetPlan(None)).await;
    }
    state.fill_side_panels(agent_id);
}

/// Point the Sandbox panel at the active agent's workspace, creating it
/// so the panel has something to show before the first tool call. A
/// cloud agent's workspace is on its machine, which the panel lists, as
/// is an agent's spawned there.
pub(crate) async fn send_sandbox_root(state: &LoopState<'_>) {
    if state.cloud.machine_of(state.active).is_some() {
        let root = PathBuf::from(aura_engine_cloud::machine_env::WORKSPACE_DIR);
        let _ = state.commands.send(UiCommand::SetSandboxRemote(root)).await;
        return;
    }
    let Some(kernel) = state.kernel_for(state.active) else {
        return;
    };
    let workspace = kernel.agent_workspace();
    if let Err(e) = tokio::fs::create_dir_all(&workspace).await {
        warn!(path = %workspace.display(), error = %e, "Failed to create agent workspace");
    }
    let _ = state
        .commands
        .send(UiCommand::SetSandboxRoot(workspace))
        .await;
}

pub(crate) async fn send_agent_list(state: &LoopState<'_>) {
    state.hub.send_agent_list().await;
}

pub(super) async fn show_error(state: &LoopState<'_>, message: String) {
    let _ = state.commands.send(UiCommand::ShowError(message)).await;
}

pub(super) async fn show_success(state: &LoopState<'_>, message: String) {
    let _ = state.commands.send(UiCommand::ShowSuccess(message)).await;
}
