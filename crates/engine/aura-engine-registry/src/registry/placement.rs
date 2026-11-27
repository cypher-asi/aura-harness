//! Channel members and coordinators: where agents a channel or hub
//! creates are placed.

use aura_core_types::AgentId;
use aura_store_db::AgentStore;
use std::path::PathBuf;

use super::{
    code, persist, AgentPlacement, AgentRegistry, AgentRegistryError, CoordinatorHome,
    RegisteredAgent,
};
use crate::error::chain;

impl AgentRegistry {
    /// Tell the registry these agents are channel members: removing their
    /// project keeps them, and they stay listed (see `is_listed`).
    pub fn mark_channel_members(&mut self, members: impl IntoIterator<Item = AgentId>) {
        self.channel_members.extend(members);
    }

    /// A fresh channel coordinator, working in `home`, not yet
    /// registered: an instance of
    /// [`crate::definitions::COORDINATOR_DEFINITION`] named just
    /// `coordinator`, as the channel it runs already says which one it
    /// is. Its code is given by [`Self::adopt`]. For a
    /// [`CoordinatorHome::Folder`] the launch project is made in memory
    /// when missing, as for other new agents, and saved with the next
    /// write. A [`CoordinatorHome::Project`] is not checked here: the
    /// caller makes sure the project is listed.
    pub fn coordinator_draft(&mut self, home: CoordinatorHome) -> RegisteredAgent {
        let placed = self.home_of(home);
        let definition = crate::definitions::COORDINATOR_DEFINITION;
        RegisteredAgent {
            definition: Some(definition.to_string()),
            project_id: placed.0,
            folder: placed.1,
            ..Self::new_agent(definition.to_string())
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
    pub fn place_agents<T, E: From<AgentRegistryError>>(
        &mut self,
        placements: &[AgentPlacement],
        write: impl FnOnce(&AgentStore, Vec<aura_store_db::AgentRecord>) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut changed = Vec::with_capacity(placements.len());
        for placement in placements {
            let (index, mut agent) = self.registered(placement.agent_id)?;
            agent.project_id.clone_from(&placement.project_id);
            agent.folder.clone_from(&placement.folder);
            changed.push((index, agent));
        }
        self.write_changed(changed, write)
    }

    /// Make the agents `agents` top level: no longer under the agents
    /// that spawned them, each keeping the folder it works in now as its
    /// own. Their rows are written by `write`, and memory changes only
    /// once it succeeded, as in [`Self::place_agents`].
    ///
    /// # Errors
    /// Returns an error if an agent is not registered (nothing is
    /// written), or `write`'s error (nothing changes in memory).
    pub fn promote_agents<T, E: From<AgentRegistryError>>(
        &mut self,
        agents: &[AgentId],
        write: impl FnOnce(&AgentStore, Vec<aura_store_db::AgentRecord>) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut changed = Vec::with_capacity(agents.len());
        for &agent_id in agents {
            let folder = self.project_folder(agent_id);
            let (index, mut agent) = self.registered(agent_id)?;
            agent.parent = None;
            agent.folder = agent.folder.or(folder);
            changed.push((index, agent));
        }
        self.write_changed(changed, write)
    }

    /// Where `agent_id` is in the registry, and a copy of it to change.
    fn registered(
        &self,
        agent_id: AgentId,
    ) -> Result<(usize, RegisteredAgent), AgentRegistryError> {
        self.position(agent_id)
            .and_then(|index| Some((index, self.file.agents.get(index)?.clone())))
            .ok_or(AgentRegistryError::NotRegistered { agent_id })
    }

    /// Have `write` store the rows of the `changed` agents (index and
    /// agent as it will be), then put them in the registry; see
    /// [`Self::place_agents`].
    fn write_changed<T, E>(
        &mut self,
        changed: Vec<(usize, RegisteredAgent)>,
        write: impl FnOnce(&AgentStore, Vec<aura_store_db::AgentRecord>) -> Result<T, E>,
    ) -> Result<T, E> {
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
                error = %chain(&error),
                "could not save the registry after placing agents; saved with the next write"
            );
        }
        Ok(written)
    }

    /// A fresh instance of `definition` joining the channel the agent
    /// `coordinator` runs, not yet registered: at the top of the
    /// coordinator's project, working in the folder the coordinator works
    /// in now, pinned as its own so a later move of the channel leaves its
    /// work where it is. Registered by [`Self::add_with`].
    pub fn member_draft(&self, definition: &str, coordinator: AgentId) -> RegisteredAgent {
        RegisteredAgent {
            project_id: self.find(coordinator).and_then(|c| c.project_id.clone()),
            folder: self.project_folder(coordinator),
            ..self.new_instance(definition)
        }
    }

    /// Register `agent`, not yet registered, with its row written by
    /// `write` (given the registry's store and the row) so a caller can
    /// commit it in one batch with rows of another store. The agent joins
    /// the registry ([`Self::adopt`]) only once `write` succeeded.
    ///
    /// # Errors
    /// `write`'s error; the registry is left as it was.
    pub fn add_with<T, E>(
        &mut self,
        agent: RegisteredAgent,
        write: impl FnOnce(&AgentStore, aura_store_db::AgentRecord) -> Result<T, E>,
    ) -> Result<(RegisteredAgent, T), E> {
        let row = self.record_for(&agent);
        let written = write(&self.store, row)?;
        Ok((self.adopt(agent), written))
    }

    /// Register `agent`, whose row is already stored (as a channel's
    /// coordinator or new member is), giving it its code, and save the
    /// code. A failed save is only logged: the agent is registered, its
    /// row is in the store already, and the code goes out with the next
    /// save. Failing here would leave a stored channel or member the app
    /// does not know until a restart.
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
                error = %chain(&error),
                "could not save the code of a new agent; saved with the next write"
            );
        }
        agent
    }
}
