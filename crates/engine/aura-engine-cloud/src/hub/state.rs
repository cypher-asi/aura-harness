//! The hub's placements, owners, plan and repo credentials, and the nodes
//! of placed agents.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::fleet::{MachineView, VmFleet};
use crate::github::{RepoRef, RepoTokens};
use crate::mesh::{Holder, HUB_NODE};
use crate::plans::{PlanCredentials, PlanStatus, PlanSync};
use crate::repos::{RepoStatus, RepoSync};
use crate::{names, BootReporter, CloudError, CloudResult};
use aura_agent::checkpoint::Checkpoints;
use aura_agent_kernel::RecordObserver;
use aura_config::CloudConfig;
use aura_core_auth::Owner;
use aura_core_types::{AgentId, Placement};

use super::node_client::NodeClient;

use super::{CloudHub, PlacedAgent};

impl CloudHub {
    /// The settings the hub runs with.
    #[must_use]
    pub const fn config(&self) -> &CloudConfig {
        &self.config
    }

    /// The fleet, for the HOST tab.
    #[must_use]
    pub fn fleet(&self) -> &Arc<VmFleet> {
        &self.fleet
    }

    pub(super) fn save_placements(&self) {
        let path = self.home.join("placements.json");
        let bytes = serde_json::to_vec_pretty(&*self.placements.lock()).unwrap_or_default();
        let tmp = path.with_extension("json.tmp");
        if let Err(err) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, &path)) {
            tracing::warn!(path = %path.display(), error = %err, "could not save cloud placements");
        }
    }

    /// Hand placed agents' nodes the app's session checkpoints, kept in
    /// `sessions`.
    pub fn attach_sessions(&self, sessions: Checkpoints) {
        *self.sessions.lock() = Some(sessions);
    }

    /// Show `observer` every entry copied from a node, so a cloud agent's
    /// KERNEL tab and Gate follow it as they follow a local agent.
    pub fn attach_observer(&self, observer: Arc<dyn RecordObserver>) {
        *self.observer.lock() = Some(observer);
    }

    /// Whether a copy brought entries since the last call; the app then
    /// lists the agents again.
    pub fn take_copied(&self) -> bool {
        self.copied.swap(false, Ordering::AcqRel)
    }

    /// The agents whose trace runs were copied since the last call; the
    /// app then shows their latest run again.
    pub fn take_traces_copied(&self) -> HashSet<AgentId> {
        std::mem::take(&mut *self.traces_copied.lock())
    }

    /// Note that `agent` (hex) of `project`, named `name`, runs here, so
    /// cloud agents can mail it.
    pub fn note_local(&self, agent: AgentId, name: &str, project: &str) {
        self.directory.place(
            &agent.to_hex(),
            Holder {
                node: HUB_NODE.to_string(),
                url: None,
                project: project.to_string(),
                name: name.to_string(),
                // Local agents act as the user signed in here.
                owner: names::owner_tag(&self.current_owner()),
            },
        );
        self.send_directories(project);
    }

    /// Call placed `agent` `name` from now on: in its placement, and in the
    /// directory its machine and other agents' mail name it by.
    pub fn rename(&self, agent: AgentId, name: &str) {
        let hex = agent.to_hex();
        let project = {
            let mut placements = self.placements.lock();
            let Some(placed) = placements.agents.get_mut(&hex) else {
                return;
            };
            if placed.name == name {
                return;
            }
            name.clone_into(&mut placed.name);
            placed.project.clone()
        };
        self.save_placements();
        if let Some(mut holder) = self.directory.holder(&hex) {
            name.clone_into(&mut holder.name);
            self.directory.place(&hex, holder);
        }
        self.send_directories(&project);
    }

    /// Whether `agent` runs on a cloud machine.
    #[must_use]
    pub fn is_placed(&self, agent: AgentId) -> bool {
        self.placements.lock().agents.contains_key(&agent.to_hex())
    }

    /// Where `agent` runs.
    #[must_use]
    pub fn placement_of(&self, agent: AgentId) -> Placement {
        if self.is_placed(agent) {
            Placement::Cloud
        } else {
            Placement::Local
        }
    }

    /// The placement new agents of `project` get.
    #[must_use]
    pub fn project_default(&self, project: &str) -> Placement {
        self.placements
            .lock()
            .projects
            .get(project)
            .copied()
            .unwrap_or(Placement::Local)
    }

    /// Remember `placement` as `project`'s choice for new agents.
    pub fn set_project_default(&self, project: &str, placement: Placement) {
        self.placements
            .lock()
            .projects
            .insert(project.to_string(), placement);
        self.save_placements();
    }

    /// Every placed agent.
    #[must_use]
    pub fn placed(&self) -> Vec<(String, PlacedAgent)> {
        self.placements
            .lock()
            .agents
            .iter()
            .map(|(hex, placed)| (hex.clone(), placed.clone()))
            .collect()
    }

    /// Read users' plan credentials from `credentials`: from now on
    /// placed machines can use their owner's Claude, ChatGPT and
    /// OpenRouter plans.
    pub fn set_plan_credentials(&self, credentials: Arc<dyn PlanCredentials>) {
        let install = self.placements.lock().install.clone();
        *self.plans.lock() = Some(Arc::new(PlanSync::new(
            self.fleet.provider(),
            credentials,
            install,
        )));
    }

    pub(super) fn plan_sync(&self) -> Option<Arc<PlanSync>> {
        self.plans.lock().clone()
    }

    /// Mint GitHub tokens with `tokens` from now on (`None`: no App).
    pub fn set_repo_tokens(&self, tokens: Option<Arc<dyn RepoTokens>>) {
        let install = self.placements.lock().install.clone();
        *self.repos.lock() = Arc::new(RepoSync::new(self.fleet.provider(), tokens, install));
    }

    pub(super) fn repo_sync(&self) -> Arc<RepoSync> {
        Arc::clone(&self.repos.lock())
    }

    /// The GitHub token state of `repo`'s machines, once it was synced.
    #[must_use]
    pub fn repo_status(&self, repo: &RepoRef) -> Option<RepoStatus> {
        self.repo_sync().status(repo)
    }

    /// The repositories placed agents work in.
    pub(super) fn placed_repos(&self) -> Vec<RepoRef> {
        let mut repos: Vec<RepoRef> = Vec::new();
        for placed in self.placements.lock().agents.values() {
            if let Some(repo) = &placed.repo {
                if !repos.contains(repo) {
                    repos.push(repo.clone());
                }
            }
        }
        repos
    }

    /// The user signed in here changed; local agents act as `owner`.
    pub fn set_current_owner(&self, owner: Owner) {
        *self.current_owner.lock() = owner;
    }

    /// The user signed in here.
    #[must_use]
    pub fn current_owner(&self) -> Owner {
        self.current_owner.lock().clone()
    }

    /// The user who placed `agent`, while it is placed.
    #[must_use]
    pub fn owner_of(&self, agent: AgentId) -> Option<Owner> {
        self.placements
            .lock()
            .agents
            .get(&agent.to_hex())
            .map(PlacedAgent::owner)
    }

    /// Where `agent` stands, while it is placed.
    #[must_use]
    pub fn placed_agent(&self, agent: AgentId) -> Option<PlacedAgent> {
        self.placements.lock().agents.get(&agent.to_hex()).cloned()
    }

    /// The display name of the user who placed `agent`.
    #[must_use]
    pub fn owner_name_of(&self, agent: AgentId) -> Option<String> {
        self.placements
            .lock()
            .agents
            .get(&agent.to_hex())
            .and_then(|placed| placed.owner_name.clone())
    }

    /// Every user with a placed agent.
    #[must_use]
    pub fn owners(&self) -> Vec<Owner> {
        let mut owners: Vec<Owner> = Vec::new();
        for placed in self.placements.lock().agents.values() {
            let owner = placed.owner();
            if !owners.contains(&owner) {
                owners.push(owner);
            }
        }
        owners
    }

    /// `owner`'s plans as their cloud machines see them.
    #[must_use]
    pub fn plan_status(&self, owner: &Owner) -> Vec<PlanStatus> {
        self.plan_sync()
            .map(|plans| plans.status(owner))
            .unwrap_or_default()
    }

    /// Push `owner`'s current plan tokens to boxd now (they linked or
    /// re-linked a plan).
    pub async fn sync_plans(&self, owner: &Owner) {
        if let Some(plans) = self.plan_sync() {
            plans.tick(std::slice::from_ref(owner)).await;
        }
    }

    /// Stop `owner`'s machines using their `kind` plan (they signed out
    /// of it).
    ///
    /// # Errors
    ///
    /// When boxd refuses the update.
    pub async fn revoke_plan(
        &self,
        owner: &Owner,
        kind: aura_core_auth::ProviderKind,
    ) -> CloudResult<()> {
        match self.plan_sync() {
            Some(plans) => plans.revoke(owner, kind).await,
            None => Ok(()),
        }
    }

    pub(super) fn place_in_directory(&self, hex: &str, agent: &PlacedAgent, view: &MachineView) {
        self.directory.place(
            hex,
            Holder {
                node: view.machine.name.clone(),
                url: Some(view.machine.url()),
                project: agent.project.clone(),
                name: agent.name.clone(),
                owner: names::owner_tag(&agent.owner()),
            },
        );
        self.send_directories(&agent.project);
    }

    pub(super) fn node(&self, agent: &str) -> CloudResult<NodeClient> {
        let view = self.fleet.view(agent).ok_or_else(|| CloudError::NotFound {
            what: "machine",
            name: agent.to_string(),
        })?;
        let token = self
            .fleet
            .node_token(agent)
            .ok_or_else(|| CloudError::BadState {
                machine: view.machine.name.clone(),
                state: view.state.label().to_string(),
                op: "call its node before it is claimed",
            })?;
        Ok(NodeClient::new(
            view.machine.name.clone(),
            view.machine.url(),
            token,
        ))
    }

    pub(super) async fn awake_node(
        &self,
        agent: &str,
        boot: &BootReporter,
    ) -> CloudResult<NodeClient> {
        self.fleet.ensure_awake_reported(agent, boot).await?;
        self.node(agent)
    }
}
