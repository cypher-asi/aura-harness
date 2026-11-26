//! What a cloud agent's machine holds and runs, read for the panels here:
//! its files (Sandbox), its plans, and its work as PROCESSES rows.

use super::*;

/// List `dir` of the on-screen cloud agent's machine for its Sandbox
/// panel. A sleeping machine is not read, so looking never wakes it.
pub(in crate::event_loop) fn list_sandbox(state: &LoopState<'_>, dir: PathBuf) {
    let agent = state.active;
    let Some(hub) = state.cloud.hub().filter(|_| state.cloud.is_cloud(agent)) else {
        return;
    };
    let ui = AgentUi::new(state.commands.clone(), agent);
    tokio::spawn(async move {
        let entries = match hub.list_files(agent, &dir).await {
            Ok(entries) => Ok(entries
                .into_iter()
                .map(|entry| RemoteEntry {
                    name: entry.name,
                    is_dir: entry.is_dir,
                })
                .collect()),
            Err(err) => {
                tracing::debug!(%agent, error = %err, "could not list the machine's files");
                Err(err.to_string())
            }
        };
        ui.send(UiCommand::SandboxListing { dir, entries }).await;
    });
}

/// Read `path` of the on-screen cloud agent's machine, which the user
/// opened. A sleeping machine is not read.
pub(in crate::event_loop) fn read_sandbox_file(state: &LoopState<'_>, path: PathBuf) {
    let agent = state.active;
    let Some(hub) = state.cloud.hub().filter(|_| state.cloud.is_cloud(agent)) else {
        return;
    };
    let ui = AgentUi::new(state.commands.clone(), agent);
    tokio::spawn(async move {
        let text = hub
            .read_file(agent, &path)
            .await
            .map_err(|err| err.to_string());
        ui.send(UiCommand::SandboxFile { path, text }).await;
    });
}

/// A cloud agent's plans, which are on its machine: its Plans tab and plan
/// strip show them as read there, or while the machine sleeps, as last
/// read from it.
pub(in crate::event_loop) struct MachinePlans {
    hub: Arc<CloudHub>,
    shared: Arc<Shared>,
    store: Arc<dyn aura_store_record::ReadStore>,
    agent: AgentId,
    /// Its project folder here, whose task board links tasks to its plans
    workspace: PathBuf,
}

impl MachinePlans {
    /// `agent`'s plans, when it runs in the cloud.
    pub(in crate::event_loop) fn of(state: &LoopState<'_>, agent: AgentId) -> Option<Self> {
        if !state.cloud.is_cloud(agent) {
            return None;
        }
        Some(Self {
            hub: state.cloud.hub()?,
            shared: Arc::clone(&state.cloud.shared),
            store: Arc::clone(&state.hub.factory.store),
            agent,
            workspace: lock(&state.hub.registry).project_folder(agent)?,
        })
    }

    /// Where its task board is.
    pub(in crate::event_loop) fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Read them and show them. Its plan in the strip is the one its
    /// copied record last wrote.
    pub(in crate::event_loop) async fn send(&self, ui: &AgentUi) {
        let (store, agent) = (Arc::clone(&self.store), self.agent);
        let active = tokio::task::spawn_blocking(move || {
            crate::record_loader::latest_plan_path(store.as_ref(), agent)
        })
        .await
        .ok()
        .flatten();
        let plans = match self.hub.machine_plans(agent, active.as_deref()).await {
            Ok(plans) => {
                self.shared.plans.lock().insert(agent, plans.clone());
                plans
            }
            Err(err) => {
                tracing::debug!(%agent, error = %err, "plans not read from the machine");
                self.shared
                    .plans
                    .lock()
                    .get(&agent)
                    .cloned()
                    .unwrap_or_default()
            }
        };
        let workspace = self.workspace.clone();
        let views = tokio::task::spawn_blocking(move || {
            crate::event_loop::plan_mode::machine_plan_views(&workspace, &plans, active.as_deref())
        })
        .await;
        let Ok((views, strip)) = views else {
            return;
        };
        ui.send(UiCommand::SetPlans(views)).await;
        ui.send(UiCommand::SetPlan(strip)).await;
    }
}

/// Show cloud agent `agent`'s plans as its machine holds them now, in the
/// background.
pub(in crate::event_loop) fn refresh_machine_plans(state: &LoopState<'_>, agent: AgentId) {
    if let Some(plans) = MachinePlans::of(state, agent) {
        let ui = AgentUi::new(state.commands.clone(), agent);
        tokio::spawn(async move { plans.send(&ui).await });
    }
}

/// What cloud agents' machines have under way, as PROCESSES rows.
pub(super) struct MachineRows(pub(super) std::sync::Weak<CloudHub>);

impl RemoteRows for MachineRows {
    fn rows(&self) -> Vec<ProcessSummary> {
        self.0.upgrade().map_or_else(Vec::new, |hub| {
            hub.machine_processes()
                .iter()
                .flat_map(|(agent, rows)| machine_rows(agent, rows))
                .collect()
        })
    }
}

/// A machine's process status as the PROCESSES tab shows it.
fn process_status(status: &str) -> ProcessStatus {
    match status {
        "idle" => ProcessStatus::Idle,
        "paused" => ProcessStatus::Paused,
        "done" => ProcessStatus::Exited,
        "failed" => ProcessStatus::Failed,
        _ => ProcessStatus::Running,
    }
}

/// The agents above `owner` in `parents`, nearest first; at most `limit`
/// of them, so a cycle in what a machine reports ends.
fn chain_above(
    parents: &std::collections::HashMap<&str, &str>,
    owner: &str,
    limit: usize,
) -> Vec<String> {
    let mut chain = Vec::new();
    let mut above = parents.get(owner).copied();
    while let Some(parent) = above.filter(|_| chain.len() < limit) {
        chain.push(parent.to_string());
        above = parents.get(parent).copied();
    }
    chain
}

/// `rows`, what placed `agent`'s machine has under way, as PROCESSES
/// rows. Spawned agents nest under the agent that spawned them, with the
/// ids local ones have, so MEMBERS lists them too.
pub(super) fn machine_rows(agent: &str, rows: &[MeshProcess]) -> Vec<ProcessSummary> {
    let parents: std::collections::HashMap<&str, &str> = rows
        .iter()
        .filter(|row| row.is_agent())
        .filter_map(|row| Some((row.agent_id.as_str(), row.parent.as_deref()?)))
        .collect();
    let names: std::collections::HashMap<&str, &str> = rows
        .iter()
        .filter_map(|row| Some((row.agent_id.as_str(), row.name.as_deref()?)))
        .collect();
    rows.iter()
        .map(|row| {
            let summary = ProcessSummary {
                label: row.label.clone(),
                status: process_status(&row.status),
                started_ms: row.started_ms,
                jump_agent_id: agent.to_string(),
                ..ProcessSummary::default()
            };
            if !row.is_agent() {
                return ProcessSummary {
                    id: format!("machine:{}", row.id),
                    kind: ProcessKind::Turn,
                    detail: row.label.clone(),
                    owner_agent_id: row.agent_id.clone(),
                    ..summary
                };
            }
            let owner = row.parent.clone().unwrap_or_else(|| agent.to_string());
            let chain = chain_above(&parents, &owner, rows.len());
            ProcessSummary {
                id: format!("agent:{}", row.agent_id),
                kind: ProcessKind::Agent,
                owner_name: names
                    .get(owner.as_str())
                    .map(|name| (*name).to_string())
                    .unwrap_or_default(),
                owner_agent_id: owner,
                parent_chain: chain,
                ..summary
            }
        })
        .collect()
}
