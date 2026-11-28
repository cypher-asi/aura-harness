//! Starting cloud agents with the app, and refreshing the HOST tab, the
//! footer switches and the agents cloud agents spawned on their machines.

use super::*;

/// How often the HOST tab and the footer switch are refreshed when
/// nothing wakes the publisher sooner ([`Shared::publish_now`]).
const PUBLISH_EVERY: Duration = Duration::from_secs(3);

/// Bring the hub up at start when a key is set, so placed agents keep
/// running in the cloud; otherwise just publish local host facts.
///
/// The cloud agents the hub saved are marked from the start, before the
/// hub connects and whether or not it does ([`super::offline::load_saved`]).
pub(in crate::event_loop) async fn start(state: &mut LoopState<'_>) {
    mark_saved_agents(state).await;
    if state.cloud.publisher.is_none() {
        start_publisher(state);
    }
    let configured = crate::integrations::cloud::cloud_configured();
    if configured {
        if let Err(err) = ensure_hub(state, &BootReporter::none()).await {
            warn!(error = %err, "cloud agents unavailable");
        }
    }
}

/// Mark the cloud agents the hub saved as cloud agents, before any work
/// reaches them; again is harmless.
pub(in crate::event_loop) async fn mark_saved_agents(state: &LoopState<'_>) {
    super::offline::load_saved(state).await;
}

pub(super) fn start_publisher(state: &mut LoopState<'_>) {
    if let Some(old) = state.cloud.publisher.take() {
        old.abort();
    }
    let shared = Arc::clone(&state.cloud.shared);
    let mut publisher = Publisher {
        shared: Arc::clone(&shared),
        hub: Arc::clone(&state.hub),
        commands: state.commands.clone(),
        auth: Arc::clone(&state.auth),
        sampler: None,
        spawns_seen: false,
    };
    state.cloud.publisher = Some(tokio::spawn(async move {
        let mut interval = tokio::time::interval(PUBLISH_EVERY);
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                () = shared.changed.notified() => {}
            }
            publisher.publish().await;
        }
    }));
}

/// What the HOST publisher keeps between refreshes.
struct Publisher {
    shared: Arc<Shared>,
    hub: Arc<AgentHub>,
    commands: tokio::sync::mpsc::Sender<UiCommand>,
    auth: Arc<aura_auth::AuthManager>,
    sampler: Option<aura_engine_cloud::host::HostSampler>,
    /// Whether the agents spawned on the machines were registered since
    /// this publisher started, which a hub coming up starts.
    spawns_seen: bool,
}

impl Publisher {
    /// One refresh: take in what the hub copied, then send the HOST view
    /// of the agent on screen and the placements the UI marks.
    async fn publish(&mut self) {
        let cloud = self.shared.hub.lock().clone();
        if let Some(cloud) = &cloud {
            self.take_in(cloud).await;
        }
        let on_screen = self.hub.record_panel.get();
        self.forget_copied_traces(cloud.as_ref(), on_screen).await;
        let current = self.auth.owner();
        let view = match on_screen {
            Some(agent) => {
                if let Some(cloud) = &cloud {
                    self.refresh_spawned_chat(cloud, agent).await;
                }
                let shared = &self.shared;
                build_view(
                    &self.hub,
                    cloud.as_ref(),
                    shared,
                    agent,
                    &current,
                    &mut self.sampler,
                )
                .await
            }
            None => None,
        };
        for id in marked_agents(cloud.as_ref(), &self.shared, on_screen) {
            let placement = placement_state(cloud.as_ref(), &self.shared, id, &current);
            let _ = self
                .commands
                .send(UiCommand::SetPlacement {
                    agent_id: id.to_hex(),
                    state: placement,
                })
                .await;
        }
        if let Some(view) = view {
            let _ = self.commands.send(UiCommand::SetHost(Box::new(view))).await;
        }
    }

    /// Take in what `cloud` copied: register the agents its placed agents
    /// spawned on their machines ([`spawns::adopt_machine_spawns`]), on
    /// each copy and once after the hub comes up, and list the agents
    /// again when they changed or which of those work did.
    async fn take_in(&mut self, cloud: &Arc<CloudHub>) {
        let copied = cloud.take_copied();
        let mut changed = copied;
        if copied || !self.spawns_seen {
            self.spawns_seen = true;
            let spawns = cloud.machine_spawns();
            if !spawns.is_empty() {
                let hub = Arc::clone(&self.hub);
                let adopt = move || spawns::adopt_machine_spawns(&hub, &spawns);
                changed |= tokio::task::spawn_blocking(adopt).await.unwrap_or(false);
            }
        }
        let working = cloud.working_spawns();
        {
            let mut shown = lock(&self.hub.machine_working);
            if *shown != working {
                *shown = working;
                changed = true;
            }
        }
        if changed {
            self.hub.refresh_agent_list().await;
        }
    }

    /// Show the chat of `agent`, on screen, as its copied record has it
    /// now, when the agent was spawned on a cloud machine and its record
    /// grew past the chat shown: no turn here streams into its chat.
    async fn refresh_spawned_chat(&self, cloud: &CloudHub, agent: AgentId) {
        if cloud.machine_root(agent).is_none() {
            return;
        }
        let hub = Arc::clone(&self.hub);
        let chat =
            tokio::task::spawn_blocking(move || crate::event_loop::agents::newer_chat(&hub, agent))
                .await
                .ok()
                .flatten();
        if let Some(chat) = chat {
            AgentUi::new(self.commands.clone(), agent)
                .send(UiCommand::LoadConversation(chat))
                .await;
        }
    }

    /// Drop the shown traces of agents whose runs were copied, and show
    /// the copied history of the one `on_screen`.
    async fn forget_copied_traces(
        &self,
        cloud: Option<&Arc<CloudHub>>,
        on_screen: Option<AgentId>,
    ) {
        for copied in cloud.map(|c| c.take_traces_copied()).unwrap_or_default() {
            self.hub.traces.forget_shown(copied);
            if on_screen == Some(copied) {
                self.hub.traces.send_history(copied).await;
            }
        }
    }
}

/// Every placed agent and every agent spawned on their machines, for the
/// projects list's cloud marks, and the agent `on_screen`, for its
/// footer switch.
pub(super) fn marked_agents(
    cloud: Option<&Arc<CloudHub>>,
    shared: &Shared,
    on_screen: Option<AgentId>,
) -> Vec<AgentId> {
    let mut agents: Vec<AgentId> = match cloud {
        Some(c) => c
            .placed()
            .iter()
            .filter_map(|(hex, _)| AgentId::from_hex(hex).ok())
            .chain(c.machine_spawns().into_iter().map(|(_, spawned)| spawned))
            .collect(),
        // Without the hub, the cloud agents it saved.
        None => shared.saved.lock().keys().copied().collect(),
    };
    if let Some(on_screen) = on_screen.filter(|agent| !agents.contains(agent)) {
        agents.push(on_screen);
    }
    agents
}
