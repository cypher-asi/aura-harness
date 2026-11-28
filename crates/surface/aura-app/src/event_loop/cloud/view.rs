use super::*;

/// How the user who placed `agent` is named here.
fn owner_label(hub: &CloudHub, agent: AgentId) -> String {
    hub.owner_name_of(agent)
        .unwrap_or_else(|| match hub.owner_of(agent) {
            Some(aura_core_auth::Owner::Zos { .. }) => "another user".to_string(),
            _ => "local profile".to_string(),
        })
}

/// The owner's name when `agent` was placed by someone other than
/// `current`.
fn foreign_label(
    cloud: Option<&Arc<CloudHub>>,
    agent: AgentId,
    current: &aura_core_auth::Owner,
) -> Option<String> {
    let hub = cloud?;
    let owner = hub.owner_of(agent)?;
    (owner != *current).then(|| owner_label(hub, agent))
}

/// The agent placed on the machine `agent` runs on: the one that spawned
/// it there, or `agent` itself.
fn placed_for(cloud: Option<&Arc<CloudHub>>, agent: AgentId) -> AgentId {
    cloud
        .and_then(|hub| hub.machine_root(agent))
        .unwrap_or(agent)
}

pub(super) fn placement_state(
    cloud: Option<&Arc<CloudHub>>,
    shared: &Shared,
    agent: AgentId,
    current: &aura_core_auth::Owner,
) -> PlacementState {
    let agent = placed_for(cloud, agent);
    let moving = shared.moving.lock().contains(&agent);
    let placement = match cloud.map_or(Placement::Local, |hub| hub.placement_of(agent)) {
        Placement::Local => PlacementView::Local,
        Placement::Cloud => PlacementView::Cloud,
    };
    let machine = cloud.and_then(|hub| hub.fleet().view(&agent.to_hex()));
    PlacementState {
        placement,
        moving: moving.then(|| placement.moving_label().to_string()),
        asleep: placement == PlacementView::Cloud
            && machine.as_ref().is_some_and(|view| !view.state.is_awake()),
        machine_state: machine.map(|view| view.state.label().to_string()),
        cloud_ready: cloud.is_some() || aura_config::cloud().is_configured(),
        owner: foreign_label(cloud, agent, current),
    }
}

pub(super) async fn build_view(
    hub: &Arc<AgentHub>,
    cloud: Option<&Arc<CloudHub>>,
    shared: &Shared,
    agent: AgentId,
    current: &aura_core_auth::Owner,
    sampler: &mut Option<aura_engine_cloud::host::HostSampler>,
) -> Option<HostView> {
    let (project_agents, folder) = project_agents(hub, agent);
    let placed = placed_for(cloud, agent);
    let mut view = match cloud.and_then(|c| c.fleet().view(&placed.to_hex()).map(|v| (c, v))) {
        Some((cloud, machine)) => HostView {
            agent_id: agent.to_hex(),
            ..cloud_agent_view(cloud, placed, &machine)
        },
        None => local_agent_view(cloud, agent, folder, sampler).await?,
    };
    let (rows, costs) = project_rows(hub, cloud, shared, current, &project_agents);
    view.project = rows;
    view.project_total = host_info::project_total_by_owner(&costs);
    Some(view)
}

/// The live agents of `agent`'s project, with their names, and its folder.
fn project_agents(hub: &AgentHub, agent: AgentId) -> (Vec<(AgentId, String)>, Option<PathBuf>) {
    let registry = lock(&hub.registry);
    let project = registry.project_of(agent).map(|p| p.id.clone());
    let folder = registry.project_folder(agent);
    let agents: Vec<(AgentId, String)> = registry
        .agents()
        .iter()
        .filter(|a| !a.archived && !a.removed && a.project_id == project)
        .map(|a| (a.agent_id(), a.display_name().into_owned()))
        .collect();
    (agents, folder)
}

fn cloud_agent_view(
    cloud: &CloudHub,
    agent: AgentId,
    machine: &aura_engine_cloud::fleet::MachineView,
) -> HostView {
    let mut view = host_info::cloud_view(
        agent,
        machine,
        cloud.host_stats(agent).as_ref(),
        cloud.cost(agent).as_ref(),
        &cloud.ledger_rows(agent, 20),
    );
    let owner = cloud
        .owner_of(agent)
        .unwrap_or(aura_core_auth::Owner::Local);
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
    host_info::add_owner_and_models(
        &mut view,
        &owner_label(cloud, agent),
        &cloud.plan_status(&owner),
        now,
    );
    if let Some(sync) = cloud.sync_status(agent) {
        host_info::add_sync(&mut view, &sync, now);
    }
    if let Some(placed) = cloud.placed_agent(agent) {
        if let Some(branch) = placed.branch.as_deref() {
            let token = placed
                .repo
                .as_ref()
                .and_then(|repo| cloud.repo_status(repo));
            host_info::add_git(
                &mut view,
                &host_info::GitFacts {
                    branch,
                    repo: placed.repo.as_ref(),
                    token: token.as_ref(),
                    fetched: placed.fetched_tip.as_deref(),
                },
                now,
            );
        }
    }
    view
}

/// This machine's view, sampled off the runtime with `sampler` (made on
/// first use, over the project `folder`'s disk).
async fn local_agent_view(
    cloud: Option<&Arc<CloudHub>>,
    agent: AgentId,
    folder: Option<PathBuf>,
    sampler: &mut Option<aura_engine_cloud::host::HostSampler>,
) -> Option<HostView> {
    let disk = folder.unwrap_or_else(|| PathBuf::from("."));
    let mut taken = sampler
        .take()
        .unwrap_or_else(|| aura_engine_cloud::host::HostSampler::new(disk, None));
    let (back, stats) = tokio::task::spawn_blocking(move || {
        let stats = taken.sample();
        (taken, stats)
    })
    .await
    .ok()?;
    *sampler = Some(back);
    let ready = cloud.is_some() || aura_config::cloud().is_configured();
    let key_file = aura_config::env_file::report()
        .and_then(|r| r.loaded_files().last().map(|p| p.display().to_string()));
    Some(host_info::local_view(
        agent,
        &stats,
        ready,
        key_file.as_deref(),
    ))
}

/// The project rows, and each cloud agent's cost by its owner's label.
fn project_rows(
    hub: &AgentHub,
    cloud: Option<&Arc<CloudHub>>,
    shared: &Shared,
    current: &aura_core_auth::Owner,
    project_agents: &[(AgentId, String)],
) -> (
    Vec<HostRow>,
    Vec<(String, aura_engine_cloud::cost::CostEstimate)>,
) {
    let mut costs = Vec::new();
    let rows = project_agents
        .iter()
        .map(|(id, name)| {
            let placed = placed_for(cloud, *id);
            match cloud.and_then(|c| c.fleet().view(&placed.to_hex()).map(|v| (c, v))) {
                Some((cloud, machine)) => {
                    // A machine's cost is counted once, on the agent placed on it.
                    let cost = (placed == *id).then(|| cloud.cost(*id)).flatten();
                    if let Some(cost) = &cost {
                        costs.push((owner_label(cloud, *id), cost.clone()));
                    }
                    host_info::cloud_row(
                        *id,
                        name,
                        &machine,
                        cost.as_ref(),
                        foreign_label(Some(cloud), placed, current),
                    )
                }
                None => host_info::local_row(
                    *id,
                    name,
                    hub.is_running(*id) || shared.running.lock().contains_key(id),
                ),
            }
        })
        .collect::<Vec<HostRow>>();
    (rows, costs)
}
