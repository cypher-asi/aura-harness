use super::*;

/// Prompts and commands for a cloud agent, or for one being moved.
/// Returns `false` when `work` is for the local kernel after all.
pub(in crate::event_loop) async fn submit(
    state: &mut LoopState<'_>,
    agent: AgentId,
    work: Work,
) -> Option<Work> {
    if state.cloud.is_moving(agent) {
        system(
            state,
            agent,
            "This agent is moving between machines; send it again when the move is done.",
        )
        .await;
        return None;
    }
    if state.cloud.take_stale(agent) {
        state.idle.remove(&agent);
    }
    if !state.cloud.is_cloud(agent) {
        return Some(work);
    }
    let hub = state.cloud.hub()?;
    if let Some(refusal) = foreign_owner(state, &hub, agent) {
        system(state, agent, refusal).await;
        return None;
    }
    match work {
        Work::Prompt { prompt, shown } => run_prompt(state, hub, agent, prompt, shown).await,
        Work::Command { command, .. } => run_command(state, hub, agent, command),
        Work::Compact { .. } | Work::Build { .. } => {
            system(state, agent, "Cloud agents summarize and build plans on their own machine; send it as a message instead.").await;
        }
    }
    None
}

/// Send `prompt` to cloud agent `agent` as a turn, unless one is running
/// or its model's plan is not linked here.
async fn run_prompt(
    state: &mut LoopState<'_>,
    hub: Arc<CloudHub>,
    agent: AgentId,
    prompt: crate::event_loop::turns::Prompt,
    shown: bool,
) {
    if state.cloud.shared.running.lock().contains_key(&agent) {
        system(
            state,
            agent,
            "The cloud agent is still working; wait or stop it first.",
        )
        .await;
        return;
    }
    let ui = AgentUi::new(state.commands.clone(), agent);
    if !shown {
        ui.send(UiCommand::ShowMessage(MessageData::new(
            MessageRole::User,
            prompt.text.clone(),
        )))
        .await;
    }
    if !prompt.images.is_empty() {
        system(
            state,
            agent,
            "Images stay on this machine; the cloud agent gets the text.",
        )
        .await;
    }
    let current = state.models.current().to_string();
    if let Some(note) = plan_refusal(state, &current) {
        system(state, agent, note).await;
        return;
    }
    let model = (!current.is_empty() && current != "gate" && !current.starts_with("local/"))
        .then_some(current);
    let jwt = zos_token(state);
    let cancel = CancellationToken::new();
    state
        .cloud
        .shared
        .running
        .lock()
        .insert(agent, cancel.clone());
    let rx = hub.run_turn(agent, prompt.text, model, jwt, cancel);
    let shared = Arc::clone(&state.cloud.shared);
    tokio::spawn(async move {
        forward(ui, rx).await;
        shared.running.lock().remove(&agent);
    });
}

/// The access token of the zOS session signed in here, if any.
fn zos_token(state: &LoopState<'_>) -> Option<String> {
    match state.auth.identity() {
        aura_auth::Identity::Zos(session) => Some(session.access_token),
        aura_auth::Identity::Local => None,
    }
}

/// Why `model` can't be used from cloud agents yet: a plan model runs on
/// the owner's plan, through the boxd secret their machine holds.
fn plan_refusal(state: &LoopState<'_>, model: &str) -> Option<String> {
    let kind = plan_of_model(model)?;
    (!state.auth.linked_plans(&state.auth.owner()).contains(&kind)).then(|| {
        format!(
            "Sign in to {} (`/login {}`) to use it from cloud agents.",
            kind.label(),
            kind.id()
        )
    })
}

/// Run `command` on cloud agent `agent`'s machine and show its output.
fn run_command(state: &LoopState<'_>, hub: Arc<CloudHub>, agent: AgentId, command: String) {
    let ui = AgentUi::new(state.commands.clone(), agent);
    tokio::spawn(async move {
        let note = match hub.exec(agent, &command).await {
            Ok(out) => format!(
                "$ {command}  (cloud, exit {})\n{}{}",
                out.exit_code,
                out.stdout_text(),
                out.stderr_tail(4000)
            ),
            Err(err) => format!("$ {command}: {err}"),
        };
        ui.send(UiCommand::ShowMessage(MessageData::new(
            MessageRole::System,
            note,
        )))
        .await;
    });
}

/// Show a cloud turn's events in its agent's chat.
pub(super) async fn forward(ui: AgentUi, mut rx: tokio::sync::mpsc::Receiver<RemoteEvent>) {
    let mut chat = TurnChat {
        ui,
        streaming: false,
        thinking: false,
        waking: None,
    };
    while let Some(event) = rx.recv().await {
        if let RemoteEvent::Boot(event) = event {
            chat.show_boot(event).await;
            continue;
        }
        chat.end_wake(&event).await;
        chat.close_open(&event).await;
        if !chat.show(event).await {
            break;
        }
    }
    chat.finish().await;
}

/// What a cloud turn has open in its chat.
struct TurnChat {
    ui: AgentUi,
    streaming: bool,
    thinking: bool,
    /// A sleeping machine's wake, shown until the turn's first event.
    waking: Option<BootConsoleUi>,
}

impl TurnChat {
    async fn show_boot(&mut self, event: BootEvent) {
        let ui = &self.ui;
        self.waking
            .get_or_insert_with(|| BootConsoleUi::new(ui.clone(), "Waking cloud machine"))
            .show(vec![event])
            .await;
    }

    /// The turn's first event ends the wake's console.
    async fn end_wake(&mut self, event: &RemoteEvent) {
        if let Some(console) = self.waking.take() {
            match event {
                RemoteEvent::Failed(err) => {
                    console
                        .end(false, format!("Could not wake the machine: {err}"))
                        .await;
                }
                _ => {
                    console
                        .end(true, "Awake; the turn runs on it.".to_string())
                        .await;
                }
            }
        }
    }

    /// Close the thinking or streaming block `event` does not continue.
    async fn close_open(&mut self, event: &RemoteEvent) {
        if self.thinking && !matches!(event, RemoteEvent::Thinking(_)) {
            self.ui.send(UiCommand::FinishThinking).await;
            self.thinking = false;
        }
        if self.streaming && !matches!(event, RemoteEvent::Text(_)) {
            self.ui.send(UiCommand::FinishStreaming).await;
            self.streaming = false;
        }
    }

    /// Show `event`. Returns `false` when the turn is over.
    async fn show(&mut self, event: RemoteEvent) -> bool {
        let ui = &self.ui;
        match event {
            RemoteEvent::Status(status) => ui.send(UiCommand::SetStatus(status)).await,
            RemoteEvent::Boot(_) => {}
            RemoteEvent::Text(text) => {
                if !self.streaming {
                    ui.send(UiCommand::StartStreaming).await;
                    self.streaming = true;
                }
                ui.send(UiCommand::AppendText(text)).await;
            }
            RemoteEvent::Thinking(text) => {
                if !self.thinking {
                    ui.send(UiCommand::StartThinking).await;
                    self.thinking = true;
                }
                ui.send(UiCommand::AppendThinking(text)).await;
            }
            event @ (RemoteEvent::ToolStart { .. }
            | RemoteEvent::ToolInput { .. }
            | RemoteEvent::ToolResult { .. }) => show_tool(ui, event).await,
            RemoteEvent::Done { .. } => return false,
            RemoteEvent::Failed(err) => {
                ui.send(UiCommand::ShowError(format!(
                    "cloud turn failed: {err}{}",
                    failure_hint(&err)
                )))
                .await;
                return false;
            }
        }
        true
    }

    /// Close what the turn left open and complete the chat.
    async fn finish(self) {
        if let Some(console) = self.waking {
            console
                .end(false, "The turn ended while the machine woke.".to_string())
                .await;
        }
        if self.thinking {
            self.ui.send(UiCommand::FinishThinking).await;
        }
        if self.streaming {
            self.ui.send(UiCommand::FinishStreaming).await;
        }
        // The chat entered Processing when the prompt was sent. Status text
        // alone leaves it there, so the working line keeps saying Thinking
        // after the turn is over.
        self.ui.send(UiCommand::Complete).await;
    }
}

/// Show a tool call's start, input or result.
async fn show_tool(ui: &AgentUi, event: RemoteEvent) {
    match event {
        RemoteEvent::ToolStart { id, name } => {
            ui.send(UiCommand::ShowTool(ToolData {
                id,
                name,
                args: String::new(),
            }))
            .await;
        }
        RemoteEvent::ToolInput { id, input } => {
            ui.send(UiCommand::UpdateToolArgs { id, args: input }).await;
        }
        RemoteEvent::ToolResult {
            id,
            result,
            is_error,
        } => {
            ui.send(UiCommand::CompleteTool {
                id,
                result,
                success: !is_error,
            })
            .await;
        }
        _ => {}
    }
}

/// What to add to a failed turn's error: a refused plan token needs a
/// new sign-in here.
fn failure_hint(err: &str) -> &'static str {
    let lower = err.to_ascii_lowercase();
    if lower.contains("401") || lower.contains("unauthorized") {
        " The plan refused the machine's token: check the plan is linked here \
         (`/login <plan>`); the machine picks up a new sign-in within two minutes."
    } else {
        ""
    }
}
