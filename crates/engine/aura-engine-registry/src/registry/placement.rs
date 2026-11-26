//! Channel members and coordinators: where agents a channel or hub
//! creates are placed.

use anyhow::Context;
use aura_core_types::AgentId;
use aura_store_db::AgentStore;
use std::path::PathBuf;

use super::{code, persist, AgentPlacement, AgentRegistry, CoordinatorHome, RegisteredAgent};

impl AgentRegistry {
    /// Tell the registry these agents are channel members: removing their
    /// project keeps them, and they stay listed (see `is_listed`).
    pub fn mark_channel_members(&mut self, members: impl IntoIterator<Item = AgentId>) {
        self.channel_members.extend(members);
    }

    /// A fresh coordinator for the channel `slug`, working in `home`, not
    /// yet registered: an instance of
    /// [`crate::definitions::COORDINATOR_DEFINITION`] named
    /// `#<slug> coordinator`. Its code is given by [`Self::adopt`]. For a
    /// [`CoordinatorHome::Folder`] the launch project is made in memory
    /// when missing, as for other new agents, and saved with the next
    /// write. A [`CoordinatorHome::Project`] is not checked here: the
    /// caller makes sure the project is listed.
    pub fn coordinator_draft(&mut self, slug: &str, home: CoordinatorHome) -> RegisteredAgent {
        let placed = self.home_of(home);
        RegisteredAgent {
            definition: Some(crate::definitions::COORDINATOR_DEFINITION.to_string()),
            project_id: placed.0,
            folder: placed.1,
            ..Self::new_agent(format!("#{slug} coordinator"))
        }
    }

    /// Where the coordinator `agent_id` goes to live in `home`, for
    /// [`Self::place_agents`]. As in [`Self::coordinator_draft`], a
    /// [`CoordinatorHome::Folder`] may make the launch project in memory.
    pub fn coordinator_placement(
        &mut self,
        agent_id: AgentId,
        home: CoordinatorHome,
    ) -> AgentPlacement {
        let (project_id, folder) = self.home_of(home);
        AgentPlacement {
            agent_id,
            project_id,
            folder,
        }
    }

    /// The project and own folder of an agent living in `home`.
    fn home_of(&mut self, home: CoordinatorHome) -> (Option<String>, Option<PathBuf>) {
        match home {
            CoordinatorHome::Project(project_id) => (Some(project_id), None),
            CoordinatorHome::Folder(folder) => (Some(self.launch_project()), Some(folder)),
        }
    }

    /// Move the agents `placements` name to their project and folder, with
    /// their rows written by `write` (given the registry's store and the
    /// rows as they will be) rather than by the registry, so a caller can
    /// commit them in one batch with rows of another store.
    ///
    /// Memory changes only once `write` succeeded: the rows are computed
    /// from copies first. Then the rows are remembered as stored, so the
    /// next save does not write them again, and whatever else is unsaved
    /// (such as a launch project [`Self::coordinator_placement`] made) is
    /// saved; a failure of that save is only logged, as the placements
    /// are in the store and the rest goes out with the next save.
    ///
    /// # Errors
    /// Returns an error if an agent is not registered (nothing is
    /// written), or `write`'s error (nothing changes in memory).
    pub fn place_agents<T>(
        &mut self,
        placements: &[AgentPlacement],
        write: impl FnOnce(&AgentStore, Vec<aura_store_db::AgentRecord>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut changed = Vec::with_capacity(placements.len());
        for placement in placements {
            let index = self
                .position(placement.agent_id)
                .with_context(|| format!("{} is not registered", placement.agent_id.to_hex()))?;
            let Some(agent) = self.file.agents.get(index) else {
                anyhow::bail!("{} is not registered", placement.agent_id.to_hex());
            };
            let mut agent = agent.clone();
            agent.project_id.clone_from(&placement.project_id);
            agent.folder.clone_from(&placement.folder);
            changed.push((index, agent));
        }
        let rows = persist::rows_at(&self.file, &changed);
        let written = write(&self.store, rows.clone())?;
        for (index, agent) in changed {
            if let Some(slot) = self.file.agents.get_mut(index) {
                *slot = agent;
            }
        }
        self.persisted.mark_agents(&rows);
        if let Err(error) = self.persist() {
            tracing::warn!(
                agents = rows.len(),
                error = %format!("{error:#}"),
                "could not save the registry after placing agents; saved with the next write"
            );
        }
        Ok(written)
    }

    /// Register `agent`, whose row is already stored (as a channel's
    /// coordinator is), giving it its code, and save the code. A failed
    /// save is only logged: the agent is registered, its row is in the
    /// store already, and the code goes out with the next save. Failing
    /// here would leave a stored channel whose coordinator the app does
    /// not know until a restart.
    pub fn adopt(&mut self, mut agent: RegisteredAgent) -> RegisteredAgent {
        if agent.code.is_none() {
            agent.code = Some(code::assign(
                &agent,
                &self.file.agents,
                &mut self.file.prefixes,
                &mut self.file.last_numbers,
            ));
        }
        self.file.agents.push(agent.clone());
        if let Err(error) = self.persist() {
            tracing::warn!(
                agent = %agent.agent_id().to_hex(),
                error = %format!("{error:#}"),
                "could not save the code of a new coordinator; saved with the next write"
            );
        }
        agent
    }
}
