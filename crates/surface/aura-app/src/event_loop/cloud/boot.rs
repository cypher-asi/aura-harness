use super::*;

/// Numbers boot consoles, so each move or wake gets one of its own. Ids
/// also carry when they were made: kept consoles come back after a
/// restart, and must not take a new console's updates.
static NEXT_BOOT: AtomicU64 = AtomicU64::new(1);

/// A boot console in an agent's chat: what a move or a wake shows while
/// it happens. It appears on [`Self::begin`] or with the first event.
#[derive(Clone)]
pub(super) struct BootConsoleUi {
    ui: AgentUi,
    id: String,
    title: String,
    begun: Arc<AtomicBool>,
    /// The agent whose start this console shows, whose rows follow its
    /// steps ([`Self::tracking_start`])
    start: Option<(Arc<AgentHub>, AgentId)>,
}

impl BootConsoleUi {
    pub(super) fn new(ui: AgentUi, title: &str) -> Self {
        Self {
            ui,
            id: format!(
                "boot-{}-{}",
                aura_core_types::time::now_ms(),
                NEXT_BOOT.fetch_add(1, Ordering::Relaxed)
            ),
            title: title.to_string(),
            begun: Arc::default(),
            start: None,
        }
    }

    /// Also put each step this console shows on `agent`'s rows, as its
    /// start reaches it.
    pub(super) fn tracking_start(mut self, hub: Arc<AgentHub>, agent: AgentId) -> Self {
        self.start = Some((hub, agent));
        self
    }

    /// Note the plan and the stage `events` reach on the started agent's
    /// rows, and send the rows when that changed them.
    async fn track_steps(&self, events: &[BootEvent]) {
        let Some((hub, agent)) = &self.start else {
            return;
        };
        let mut changed = false;
        for event in events {
            changed |= match event {
                BootEvent::Plan(stages) => hub.status.plan_start(
                    *agent,
                    stages.iter().map(|stage| stage.key().to_string()).collect(),
                ),
                BootEvent::Stage {
                    stage,
                    state: StageState::Started,
                } => hub.status.start_stage(
                    *agent,
                    (stage.key(), stage.label()),
                    aura_core_types::time::now_ms(),
                ),
                _ => false,
            };
        }
        if changed {
            hub.send_agent_list().await;
        }
    }

    async fn send(&self, update: BootUpdate) {
        self.ui
            .send(UiCommand::Boot {
                id: self.id.clone(),
                update,
            })
            .await;
    }

    /// Show the console now.
    pub(super) async fn begin(&self) {
        if !self.begun.swap(true, Ordering::Relaxed) {
            self.send(BootUpdate::Begin {
                title: self.title.clone(),
            })
            .await;
        }
    }

    pub(super) async fn show(&self, events: Vec<BootEvent>) {
        self.begin().await;
        self.track_steps(&events).await;
        for update in boot_updates(events) {
            self.send(update).await;
        }
    }

    /// Show everything sent on `events` until its reporter is dropped.
    pub(super) fn pump(
        &self,
        mut events: tokio::sync::mpsc::UnboundedReceiver<BootEvent>,
    ) -> tokio::task::JoinHandle<()> {
        let console = self.clone();
        tokio::spawn(async move {
            let mut batch = Vec::new();
            while events.recv_many(&mut batch, 256).await > 0 {
                console.show(std::mem::take(&mut batch)).await;
            }
        })
    }

    /// End the console with `summary`. Returns `false` when it never
    /// showed, so the caller says how it went another way.
    pub(super) async fn end(&self, ok: bool, summary: String) -> bool {
        if !self.begun.load(Ordering::Relaxed) {
            return false;
        }
        self.send(BootUpdate::End { ok, summary }).await;
        true
    }
}

/// `events` as console updates, runs of log lines sent as one.
pub(super) fn boot_updates(events: Vec<BootEvent>) -> Vec<BootUpdate> {
    let mut updates = Vec::new();
    for event in events {
        let update = match event {
            BootEvent::Plan(stages) => BootUpdate::Plan(
                stages
                    .into_iter()
                    .map(|stage| BootPlanStep {
                        key: stage.key().to_string(),
                        label: stage.label().to_string(),
                    })
                    .collect(),
            ),
            BootEvent::Stage { stage, state } => BootUpdate::Stage {
                key: stage.key().to_string(),
                label: stage.label().to_string(),
                state: match state {
                    StageState::Started => BootStageState::Running,
                    StageState::Done { ms } => BootStageState::Done { ms },
                    StageState::Skipped(reason) => BootStageState::Skipped(reason),
                    StageState::Failed(reason) => BootStageState::Failed(reason),
                },
            },
            BootEvent::Fact { key, value } => BootUpdate::Fact { key, value },
            BootEvent::Log { stderr, line, .. } => {
                let line = BootLogLine { stderr, text: line };
                if let Some(BootUpdate::Log(lines)) = updates.last_mut() {
                    lines.push(line);
                    continue;
                }
                BootUpdate::Log(vec![line])
            }
            BootEvent::Progress { stage, done, total } => BootUpdate::Progress {
                key: stage.key().to_string(),
                done,
                total,
            },
            BootEvent::Waiting { stage, what, limit } => BootUpdate::Waiting {
                key: stage.key().to_string(),
                what,
                limit_ms: limit.map(|limit| u64::try_from(limit.as_millis()).unwrap_or(u64::MAX)),
            },
        };
        updates.push(update);
    }
    updates
}

/// Show `events` on a start console of `agent`, its rows following its
/// steps, as a cloud start does.
#[cfg(test)]
pub(in crate::event_loop) async fn show_start_events(
    hub: &Arc<AgentHub>,
    agent: AgentId,
    events: Vec<BootEvent>,
) {
    let ui = AgentUi::new(hub.commands().clone(), agent);
    BootConsoleUi::new(ui, "Starting in the cloud")
        .tracking_start(Arc::clone(hub), agent)
        .show(events)
        .await;
}
