//! Starting cloud agents with the app, and refreshing the HOST tab and
//! the footer switches.

use super::*;

/// How often the HOST tab and the footer switch are refreshed.
const PUBLISH_EVERY: Duration = Duration::from_secs(3);

/// Bring the hub up at start when a key is set, so placed agents keep
/// running in the cloud; otherwise just publish local host facts.
pub(in crate::event_loop) async fn start(state: &mut LoopState<'_>) {
    let configured = aura_config::cloud().is_configured();
    if configured {
        if let Err(err) = ensure_hub(state, &BootReporter::none()).await {
            warn!(error = %err, "cloud agents unavailable");
        }
    }
    if state.cloud.publisher.is_none() {
        start_publisher(state);
    }
}

pub(super) fn start_publisher(state: &mut LoopState<'_>) {
    if let Some(old) = state.cloud.publisher.take() {
        old.abort();
    }
    let mut publisher = Publisher {
        shared: Arc::clone(&state.cloud.shared),
        hub: Arc::clone(&state.hub),
        commands: state.commands.clone(),
        auth: Arc::clone(&state.auth),
        sampler: None,
    };
    state.cloud.publisher = Some(tokio::spawn(async move {
        let mut interval = tokio::time::interval(PUBLISH_EVERY);
        loop {
            interval.tick().await;
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
}

impl Publisher {
    /// One refresh: take in what the hub copied, then send the HOST view
    /// of the agent on screen and the placements the UI marks.
    async fn publish(&mut self) {
        let cloud = self.shared.hub.lock().clone();
        if cloud.as_ref().is_some_and(|cloud| cloud.take_copied()) {
            self.hub.refresh_agent_list().await;
        }
        let on_screen = self.hub.record_panel.get();
        self.forget_copied_traces(cloud.as_ref(), on_screen).await;
        let Some(agent) = on_screen else {
            return;
        };
        let current = self.auth.owner();
        let view = build_view(
            &self.hub,
            cloud.as_ref(),
            &self.shared,
            agent,
            &current,
            &mut self.sampler,
        )
        .await;
        for id in marked_agents(cloud.as_ref(), agent) {
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

/// Every placed agent, for the projects list's cloud marks, and the agent
/// `on_screen`, for its footer switch.
fn marked_agents(cloud: Option<&Arc<CloudHub>>, on_screen: AgentId) -> Vec<AgentId> {
    let mut agents: Vec<AgentId> = cloud
        .map(|c| {
            c.placed()
                .iter()
                .filter_map(|(hex, _)| AgentId::from_hex(hex).ok())
                .collect()
        })
        .unwrap_or_default();
    if !agents.contains(&on_screen) {
        agents.push(on_screen);
    }
    agents
}
