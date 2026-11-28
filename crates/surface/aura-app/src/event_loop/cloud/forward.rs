//! A cloud turn's stream in its agent's chat, and the chat title the turn
//! gives it. A stream that shows only how the turn ends (one attached to
//! again, or a turn followed after Aura restarted) has the chat rebuilt
//! from the record as the hub copies it.

pub(super) use super::turn_marks::WaitingMark;
use super::turn_marks::{wakes, WAKING};
use super::*;

/// Applies a cloud chat's title.
pub(super) type TitleSink = Box<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync>;

/// Titles `agent` in the agents list and, when that took, on its placement,
/// which names it in mail.
pub(super) fn title_sink(agents: Arc<AgentHub>, hub: Arc<CloudHub>, agent: AgentId) -> TitleSink {
    Box::new(move |title| {
        let agents = Arc::clone(&agents);
        let hub = Arc::clone(&hub);
        Box::pin(async move {
            if agents.apply_title(agent, &title).await {
                hub.rename(agent, &title);
            }
        })
    })
}

/// Where a cloud chat's title goes, with the title made from its prompt
/// for when the machine sends none.
pub(super) struct Titles {
    sink: TitleSink,
    fallback: Option<String>,
}

impl Titles {
    pub(super) fn new(sink: TitleSink, prompt: &str) -> Self {
        Self {
            sink,
            fallback: Some(aura_agent::titles::fallback_title(prompt)),
        }
    }

    async fn apply(&mut self, title: String) {
        self.fallback = None;
        (self.sink)(title).await;
    }

    /// Apply the title made from the prompt, unless one was applied.
    pub(super) async fn finish(&mut self) {
        if let Some(title) = self.fallback.take() {
            (self.sink)(title).await;
        }
    }
}

/// Shows a cloud chat as its copied record has it now.
pub(super) type ChatReload = Box<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

/// Shows `agent`'s chat in `ui` as its record has it, when the record
/// grew past the chat last shown.
pub(super) fn chat_reload(agents: Arc<AgentHub>, ui: AgentUi, agent: AgentId) -> ChatReload {
    Box::new(move || {
        let agents = Arc::clone(&agents);
        let ui = ui.clone();
        Box::pin(async move {
            let read = move || crate::event_loop::agents::newer_chat(&agents, agent);
            if let Some(chat) = tokio::task::spawn_blocking(read).await.ok().flatten() {
                ui.send(UiCommand::LoadConversation(chat)).await;
            }
        })
    })
}

/// What a cloud turn shows outside its chat's stream.
#[derive(Default)]
pub(super) struct TurnExtras {
    /// Where the chat's title goes; `None` when it needs none.
    pub(super) titles: Option<Titles>,
    /// The context window of the model the turn asked for.
    pub(super) context_limit: Option<u64>,
    /// Shows the chat from the record, once the turn's stream shows only
    /// how it ends ([`RemoteEvent::Copied`]).
    pub(super) reload: Option<ChatReload>,
    /// Whether the chat shows the turn from its record: a turn attached
    /// to again, or followed after Aura restarted, streams no reply.
    pub(super) from_record: bool,
    /// Where the turn notes, for the agent's rows and its peers, that it
    /// waits for the one already under way on its machine
    pub(super) waiting: Option<WaitingMark>,
    /// Set on a turn an earlier run of Aura asked for: when it turns out
    /// cut short, its agent carries on instead of giving up what it owes
    pub(super) cut_short: Option<CutShort>,
    /// The reply text the turn streamed since its last tool call: its
    /// last words once it ends, which answer what it was asked
    pub(super) said: String,
    /// Why the turn's conversation could not be copied here, once the
    /// hub says so ([`RemoteEvent::CopyFailed`])
    pub(super) copy_failed: Option<String>,
}

/// How an agent whose followed turn was cut short carries on: the
/// harness notice it is sent, which starts its next turn once the loop
/// frees it. What it owes stays owed.
pub(super) struct CutShort {
    pub(super) notices: crate::event_loop::notices::NoticeSender,
    pub(super) agent: AgentId,
    /// The prompt asking it to carry on
    pub(super) resume: String,
}

/// What the chat says as an agent cut short carries on.
const CARRIES_ON: &str = "↻ Carrying on after the app restarted";

impl CutShort {
    /// Ask the agent to carry on.
    pub(super) fn carry_on(&self) {
        let notice = crate::event_loop::notices::Notice {
            to: self.agent,
            shown: CARRIES_ON.to_string(),
            prompt: self.resume.clone(),
            inbox: None,
        };
        if self.notices.send(notice).is_err() {
            tracing::warn!(agent = %self.agent, "The terminal is shutting down; the agent is not asked to carry on");
        }
    }
}

impl TurnExtras {
    /// Show the chat from the record from now on, as it has it now.
    pub(super) async fn show_record(&mut self) {
        self.from_record = true;
        if let Some(reload) = &self.reload {
            reload().await;
        }
    }
}

/// [`forward`] a turn of `agent` from `rx` to `ui`, noting in `hub` while
/// it waits on its machine, as a cloud turn does: for tests outside this
/// module.
#[cfg(test)]
pub(in crate::event_loop) async fn forward_marking_waits(
    hub: Arc<AgentHub>,
    agent: AgentId,
    ui: &AgentUi,
    rx: &mut tokio::sync::mpsc::Receiver<RemoteEvent>,
) {
    let mut extras = TurnExtras {
        waiting: Some(WaitingMark { hub, agent }),
        ..TurnExtras::default()
    };
    forward(ui, rx, &mut extras, &CancellationToken::new()).await;
}

/// How a cloud turn's stream ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TurnEnd {
    /// The reply ended.
    Done,
    /// The turn failed, or its stream closed first, and why.
    Failed(String),
}

/// Why a turn whose stream closed before the reply ended stopped.
pub(super) const CLOSED_EARLY: &str = "its machine stopped answering before the turn ended";

impl TurnEnd {
    /// The PROCESSES status of a turn that ended so, when the user
    /// `stopped` it or not.
    pub(super) const fn status(&self, stopped: bool) -> ProcessStatus {
        match (self, stopped) {
            (_, true) => ProcessStatus::Killed,
            (Self::Done, false) => ProcessStatus::Exited,
            (Self::Failed(_), false) => ProcessStatus::Failed,
        }
    }
}

/// Show a cloud turn's events in its agent's chat, which works on the
/// turn until it ends. A turn ended by `cancel` says so, without an error.
pub(super) async fn forward(
    ui: &AgentUi,
    rx: &mut tokio::sync::mpsc::Receiver<RemoteEvent>,
    extras: &mut TurnExtras,
    cancel: &CancellationToken,
) -> TurnEnd {
    // A prompt typed into the chat put it to work already; one from the
    // channel or the queue did not, and an idle chat folds each group of
    // steps as soon as none of them runs.
    ui.send(UiCommand::TurnStarted { prompt: None }).await;
    let mut chat = TurnChat {
        ui,
        cancel,
        streaming: false,
        thinking: false,
        waking: None,
        reconnect: false,
    };
    let mut end = TurnEnd::Failed(CLOSED_EARLY.to_string());
    while let Some(event) = rx.recv().await {
        if let RemoteEvent::Boot(event) = event {
            chat.show_boot(event).await;
            continue;
        }
        // Arrives mid-reply; the reply streams on around it.
        if let RemoteEvent::Title(title) = event {
            if let Some(titles) = extras.titles.as_mut() {
                titles.apply(title).await;
            }
            continue;
        }
        if let Some(mark) = &extras.waiting {
            mark.see(&event).await;
        }
        chat.end_wake(&event).await;
        chat.close_open(&event).await;
        // A failed turn's copy is tried before its failure is sent.
        if let RemoteEvent::CopyFailed(why) = event {
            extras.copy_failed = Some(why);
            continue;
        }
        if matches!(event, RemoteEvent::Copied) {
            extras.show_record().await;
            continue;
        }
        note_said(&mut extras.said, &event);
        if let Some(ended) = chat.show(event, extras.context_limit).await {
            end = ended;
            break;
        }
    }
    if let Some(mark) = &extras.waiting {
        mark.set(false).await;
    }
    chat.finish(extras).await;
    end
}

/// Keep in `said` the reply text `event` adds: a tool call starts the
/// turn's words afresh, so what is left at its end is its last message.
fn note_said(said: &mut String, event: &RemoteEvent) {
    match event {
        RemoteEvent::Text(text) => said.push_str(text),
        RemoteEvent::ToolStart { .. } => said.clear(),
        _ => {}
    }
}

/// What a cloud turn has open in its chat.
struct TurnChat<'a> {
    ui: &'a AgentUi,
    cancel: &'a CancellationToken,
    streaming: bool,
    thinking: bool,
    /// A sleeping machine's wake, or a reconnect to a running one, shown
    /// until the turn's first event.
    waking: Option<BootConsoleUi>,
    /// Whether that console is a reconnect rather than a wake.
    reconnect: bool,
}

impl TurnChat<'_> {
    /// Show a boot step of the machine in a console, which says
    /// "Waking cloud machine" when the steps begin with a wake and
    /// "Reconnecting…" when the machine is already up and only its node
    /// is claimed again (a turn picked up after a restart).
    async fn show_boot(&mut self, event: BootEvent) {
        if self.waking.is_none() {
            self.reconnect = !wakes(&event);
            let title = if self.reconnect {
                crate::cloud::hub::RECONNECTING
            } else {
                WAKING
            };
            self.waking = Some(BootConsoleUi::new(self.ui.clone(), title));
        }
        if let Some(console) = &self.waking {
            console.show(vec![event]).await;
        }
    }

    /// The turn's first event ends the wake's console.
    async fn end_wake(&mut self, event: &RemoteEvent) {
        let Some(console) = self.waking.take() else {
            return;
        };
        match event {
            RemoteEvent::Failed(_) if self.cancel.is_cancelled() => {
                console
                    .end(false, "The turn was stopped.".to_string())
                    .await;
            }
            RemoteEvent::Failed(err) if self.reconnect => {
                console
                    .end(false, format!("Could not reconnect to the machine: {err}"))
                    .await;
            }
            RemoteEvent::Failed(err) => {
                console
                    .end(false, format!("Could not wake the machine: {err}"))
                    .await;
            }
            _ if self.reconnect => {
                console
                    .end(true, "Reconnected; its turn runs on.".to_string())
                    .await;
            }
            _ => {
                console
                    .end(true, "Awake; the turn runs on it.".to_string())
                    .await;
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

    /// Show `event`. Returns how the turn ended, once it has.
    async fn show(&mut self, event: RemoteEvent, context_limit: Option<u64>) -> Option<TurnEnd> {
        let ui = self.ui;
        match event {
            RemoteEvent::Status(status) => ui.send(UiCommand::SetStatus(status)).await,
            RemoteEvent::Boot(_)
            | RemoteEvent::Title(_)
            | RemoteEvent::Copied
            | RemoteEvent::CopyFailed(_) => {}
            // `close_open` ended the reply the call wrote.
            RemoteEvent::ModelCallEnd => {}
            RemoteEvent::Text(text) => self.append_text(text).await,
            RemoteEvent::Thinking(text) => self.append_thinking(text).await,
            event @ (RemoteEvent::ToolStart { .. }
            | RemoteEvent::ToolInput { .. }
            | RemoteEvent::ToolOutput { .. }
            | RemoteEvent::ToolResult { .. }) => show_tool(ui, event).await,
            // One that ended while no app watched comes without usage.
            RemoteEvent::Done(usage) if *usage == aura_core_protocol::SessionUsage::default() => {
                return Some(TurnEnd::Done);
            }
            RemoteEvent::Done(usage) => {
                let limit = context_limit.or_else(|| {
                    (!usage.model.is_empty())
                        .then(|| aura_engine::context_window_for_model(&usage.model))
                });
                ui.send(UiCommand::SetContext(
                    crate::event_loop::hud::remote_context_view(&usage, limit),
                ))
                .await;
                return Some(TurnEnd::Done);
            }
            RemoteEvent::Failed(err) if self.cancel.is_cancelled() => {
                info!(error = %err, "Cancelled cloud turn ended with an error");
                ui.send(UiCommand::ShowWarning("Turn cancelled".to_string()))
                    .await;
                return Some(TurnEnd::Failed(err));
            }
            RemoteEvent::Failed(err) => {
                ui.send(UiCommand::ShowError(format!(
                    "cloud turn failed: {err}{}",
                    failure_hint(&err)
                )))
                .await;
                return Some(TurnEnd::Failed(err));
            }
        }
        None
    }

    async fn append_text(&mut self, text: String) {
        if !self.streaming {
            self.ui.send(UiCommand::StartStreaming).await;
            self.streaming = true;
        }
        self.ui.send(UiCommand::AppendText(text)).await;
    }

    async fn append_thinking(&mut self, text: String) {
        if !self.thinking {
            self.ui.send(UiCommand::StartThinking).await;
            self.thinking = true;
        }
        self.ui.send(UiCommand::AppendThinking(text)).await;
    }

    /// Close what the turn left open, title the chat if the machine did
    /// not, and complete the chat.
    async fn finish(self, extras: &mut TurnExtras) {
        if let Some(console) = self.waking {
            let said = if self.reconnect {
                "The turn ended while Aura reconnected to it."
            } else {
                "The turn ended while the machine woke."
            };
            console.end(false, said.to_string()).await;
        }
        if let Some(titles) = extras.titles.as_mut() {
            titles.finish().await;
        }
        if self.thinking {
            self.ui.send(UiCommand::FinishThinking).await;
        }
        if self.streaming {
            self.ui.send(UiCommand::FinishStreaming).await;
        }
        // Status text alone leaves the chat in Processing, where `forward`
        // put it, so the working line would keep saying Thinking after the
        // turn is over.
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
        RemoteEvent::ToolOutput { id, chunk } => {
            ui.send(UiCommand::ToolOutput { id, chunk }).await;
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
