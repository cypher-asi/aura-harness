//! Read access: the agents, projects and the active agent.

use aura_core_types::AgentId;
use std::path::{Path, PathBuf};

use super::{persist, AgentRegistry, Project, RegisteredAgent};

impl AgentRegistry {
    #[must_use]
    pub fn agents(&self) -> &[RegisteredAgent] {
        &self.file.agents
    }

    /// Every project, listed or not, in list order.
    #[must_use]
    pub fn projects(&self) -> &[Project] {
        &self.file.projects
    }

    #[must_use]
    pub fn find(&self, agent_id: AgentId) -> Option<&RegisteredAgent> {
        self.file.agents.iter().find(|a| a.agent_id() == agent_id)
    }

    /// The project with id `id`.
    #[must_use]
    pub fn project(&self, id: &str) -> Option<&Project> {
        self.file.projects.iter().find(|p| p.id == id)
    }

    /// The project `agent_id` is in.
    #[must_use]
    pub fn project_of(&self, agent_id: AgentId) -> Option<&Project> {
        self.project(self.find(agent_id)?.project_id.as_deref()?)
    }

    /// The agent to open on startup: the last active one, else the first
    /// listed one. Archived agents are not opened while another is listed.
    #[must_use]
    pub fn active(&self) -> &RegisteredAgent {
        let stored = self
            .file
            .active
            .as_deref()
            .and_then(|zns| self.file.agents.iter().find(|a| a.zns_id == zns));
        stored
            .filter(|agent| self.is_listed(agent))
            .or_else(|| self.tree().first().map(|(agent, _)| *agent))
            .or(stored)
            .unwrap_or(&self.file.agents[0])
    }

    /// The agent open now: the stored active agent even when it sits in
    /// the Archive, since an archived agent can be opened and chatted with
    /// without leaving it. Falls back to [`Self::active`] when the stored
    /// one is unknown or left behind by a removal.
    #[must_use]
    pub fn current(&self) -> &RegisteredAgent {
        self.file
            .active
            .as_deref()
            .and_then(|zns| self.file.agents.iter().find(|a| a.zns_id == zns))
            .filter(|agent| !agent.removed)
            .unwrap_or_else(|| self.active())
    }

    /// Remember `agent_id` as the active agent.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn set_active(&mut self, agent_id: AgentId) -> anyhow::Result<()> {
        if let Some(agent) = self.find(agent_id) {
            if self.file.active.as_deref() == Some(agent.zns_id.as_str()) {
                return Ok(());
            }
            self.file.active = Some(agent.zns_id.clone());
            self.persist()?;
        }
        Ok(())
    }

    /// Folder `agent_id` works in: its own, else the nearest one of the
    /// agents that spawned it, else its project's.
    #[must_use]
    pub fn project_folder(&self, agent_id: AgentId) -> Option<PathBuf> {
        let agent = self.find(agent_id)?;
        std::iter::once(agent)
            .chain(self.spawners(agent))
            .find_map(|a| a.folder.clone())
            .or_else(|| self.project_of(agent_id).map(|p| p.folder.clone()))
    }

    pub(super) fn position(&self, agent_id: AgentId) -> Option<usize> {
        self.file
            .agents
            .iter()
            .position(|agent| agent.agent_id() == agent_id)
    }

    /// Data directory the registry belongs to.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The row `agent`, not yet registered, has once [`Self::adopt`]
    /// appends it (before its code is assigned).
    pub fn record_for(&self, agent: &RegisteredAgent) -> aura_store_db::AgentRecord {
        persist::appended_row(&self.file, agent)
    }

    /// The store the registry is kept in. Writers outside the registry
    /// get it through [`Self::place_agents`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn store(&self) -> &aura_store_db::AgentStore {
        &self.store
    }
}
