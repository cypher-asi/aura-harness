//! Changing agents and projects: folders, titles, archiving, removing
//! and reordering.

use aura_core_types::AgentId;
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::path::PathBuf;

use super::{AgentRegistry, AgentRegistryError, RegisteredAgent};

impl AgentRegistry {
    /// Have `agent_id` work in `folder`.
    ///
    /// # Errors
    /// Returns an error if the agent is unknown or the registry cannot be
    /// written.
    pub fn set_folder(
        &mut self,
        agent_id: AgentId,
        folder: PathBuf,
    ) -> Result<(), AgentRegistryError> {
        let Some(index) = self.position(agent_id) else {
            return Err(AgentRegistryError::NoSuchAgent { agent_id });
        };
        self.file.agents[index].folder = Some(folder);
        self.persist()
    }

    /// Drop `agent_id`, registered for work that never started, from the
    /// registry altogether. An agent something spawned stays.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn forget(&mut self, agent_id: AgentId) -> Result<(), AgentRegistryError> {
        let Some(index) = self.position(agent_id) else {
            return Ok(());
        };
        let zns_id = &self.file.agents[index].zns_id;
        if self
            .file
            .agents
            .iter()
            .any(|a| a.parent.as_ref() == Some(zns_id))
        {
            return Ok(());
        }
        self.file.agents.remove(index);
        self.persist()
    }

    /// Give `agent_id` the chat title `title`. Blank titles are ignored.
    ///
    /// # Errors
    /// Returns an error if the agent is unknown or the registry cannot be
    /// written.
    pub fn set_title(&mut self, agent_id: AgentId, title: &str) -> Result<(), AgentRegistryError> {
        let title = title.trim();
        if title.is_empty() {
            return Ok(());
        }
        let Some(index) = self.position(agent_id) else {
            return Err(AgentRegistryError::NoSuchAgent { agent_id });
        };
        self.file.agents[index].title = Some(title.to_string());
        self.persist()
    }

    /// Give `agent_id` the subtask name `subtitle`. Blank subtitles are
    /// ignored.
    ///
    /// # Errors
    /// Returns an error if the agent is unknown or the registry cannot be
    /// written.
    pub fn set_subtitle(
        &mut self,
        agent_id: AgentId,
        subtitle: &str,
    ) -> Result<(), AgentRegistryError> {
        let subtitle = subtitle.trim();
        if subtitle.is_empty() {
            return Ok(());
        }
        let Some(index) = self.position(agent_id) else {
            return Err(AgentRegistryError::NoSuchAgent { agent_id });
        };
        self.file.agents[index].subtitle = Some(subtitle.to_string());
        self.persist()
    }

    /// Move `agent_id`, and the agents it spawned, to the Archive. The
    /// other agents of its project stay listed. An agent already in the
    /// Archive is left as it is, keeping its date and place there.
    ///
    /// # Errors
    /// Returns an error if the agent is unknown, it is the last listed
    /// agent, or the registry cannot be written.
    pub fn archive(&mut self, agent_id: AgentId) -> Result<(), AgentRegistryError> {
        self.archive_at(agent_id, Utc::now())
    }

    /// [`Self::archive`], dated `at`, so tests can fix the Archive order.
    pub(crate) fn archive_at(
        &mut self,
        agent_id: AgentId,
        at: DateTime<Utc>,
    ) -> Result<(), AgentRegistryError> {
        let Some(index) = self.position(agent_id) else {
            return Err(AgentRegistryError::NoSuchAgent { agent_id });
        };
        if self.file.agents[index].archived {
            return Ok(());
        }
        let hiding: HashSet<AgentId> = self.subtree(agent_id).into_iter().collect();
        if self
            .tree()
            .iter()
            .all(|(agent, _)| hiding.contains(&agent.agent_id()))
        {
            return Err(AgentRegistryError::LastAgent { agent_id });
        }
        let agent = &mut self.file.agents[index];
        agent.archived = true;
        agent.archived_at = Some(at);
        self.persist()
    }

    /// Bring `agent_id` back from the Archive, with the agents that spawned
    /// it so it has its old place in the list. No other agent of its
    /// project comes back. When its project was removed in the meantime,
    /// the project comes back holding just it.
    ///
    /// # Errors
    /// Returns an error if the agent is unknown or the registry cannot be
    /// written.
    pub fn restore(&mut self, agent_id: AgentId) -> Result<(), AgentRegistryError> {
        let Some(agent) = self.find(agent_id) else {
            return Err(AgentRegistryError::NoSuchAgent { agent_id });
        };
        let mut restoring: Vec<String> = self
            .spawners(agent)
            .iter()
            .map(|a| a.zns_id.clone())
            .collect();
        restoring.push(agent.zns_id.clone());
        let project_id = agent.project_id.clone();
        for agent in &mut self.file.agents {
            if restoring.contains(&agent.zns_id) {
                agent.archived = false;
                agent.archived_at = None;
                agent.removed = false;
            }
        }
        if let Some(project) = self
            .file
            .projects
            .iter_mut()
            .find(|p| Some(&p.id) == project_id.as_ref())
        {
            project.removed = false;
        }
        self.persist()
    }

    /// Take project `project_id` off the list with its agents. Agents in
    /// the Archive stay there, so the project and the agents that spawned
    /// them are kept, hidden, to put back when one is restored. The other
    /// agents are dropped from the registry; records and the project's
    /// folder are left alone. Returns the agents that left the list.
    ///
    /// # Errors
    /// Returns an error if the project is unknown, it holds every listed
    /// agent, or the registry cannot be written.
    pub fn remove_project(&mut self, project_id: &str) -> Result<Vec<AgentId>, AgentRegistryError> {
        if self.project(project_id).is_none() {
            return Err(AgentRegistryError::NoSuchProject {
                project_id: project_id.to_string(),
            });
        }
        let gone = self.leaving_agents(project_id)?;
        let (archived, hiding) = self.archived_of_project(project_id);
        let members = &self.channel_members;
        let in_project = |a: &RegisteredAgent| owned_by_project(a, project_id, members);
        for agent in &mut self.file.agents {
            if hiding.contains(&agent.agent_id()) {
                agent.removed = true;
            }
        }
        self.file.agents.retain(|agent| {
            let id = agent.agent_id();
            !in_project(agent) || archived.contains(&id) || hiding.contains(&id)
        });
        if let Some(project) = self.file.projects.iter_mut().find(|p| p.id == project_id) {
            project.removed = true;
        }
        self.clear_dangling_active();
        self.persist()?;
        Ok(gone)
    }

    /// The listed agents removing project `project_id` takes off the list.
    ///
    /// # Errors
    /// Returns an error if the project holds every listed agent.
    fn leaving_agents(&self, project_id: &str) -> Result<Vec<AgentId>, AgentRegistryError> {
        let members = &self.channel_members;
        // Channel members are not the project's to remove: they stay, and
        // the project's own agents decide whether it is the last one.
        let in_project = |a: &RegisteredAgent| owned_by_project(a, project_id, members);
        let tree = self.tree();
        if tree
            .iter()
            .filter(|(agent, _)| !members.contains(&agent.agent_id()))
            .all(|(agent, _)| in_project(agent))
        {
            return Err(AgentRegistryError::LastProject {
                project_id: project_id.to_string(),
            });
        }
        Ok(tree
            .iter()
            .filter(|(agent, _)| in_project(agent))
            .map(|(agent, _)| agent.agent_id())
            .collect())
    }

    /// The archived agents of project `project_id` with everything below
    /// them, and the agents that spawned them, to keep hidden.
    fn archived_of_project(&self, project_id: &str) -> (Vec<AgentId>, Vec<AgentId>) {
        let members = &self.channel_members;
        let heads: Vec<&RegisteredAgent> = self
            .archived_tree()
            .into_iter()
            .filter(|(agent, depth)| *depth == 0 && owned_by_project(agent, project_id, members))
            .map(|(agent, _)| agent)
            .collect();
        let archived: Vec<AgentId> = heads
            .iter()
            .flat_map(|head| self.subtree(head.agent_id()))
            .collect();
        let hiding: Vec<AgentId> = heads
            .iter()
            .flat_map(|head| self.spawners(head))
            .map(RegisteredAgent::agent_id)
            .collect();
        (archived, hiding)
    }

    /// Forget the active agent when it is no longer registered.
    fn clear_dangling_active(&mut self) {
        if self
            .file
            .active
            .as_deref()
            .is_some_and(|zns| !self.file.agents.iter().any(|a| a.zns_id == zns))
        {
            self.file.active = None;
        }
    }

    /// List the projects `order` names in that order, in the places they
    /// held between them. Other projects keep their places, so a view
    /// showing some of the projects can reorder just those.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn reorder_projects(&mut self, order: &[String]) -> Result<(), AgentRegistryError> {
        let slots: Vec<usize> = (0..self.file.projects.len())
            .filter(|&i| order.contains(&self.file.projects[i].id))
            .collect();
        if reorder_slots(&mut self.file.projects, &slots, order, |p| p.id.clone()) {
            self.persist()?;
        }
        Ok(())
    }

    /// List the top-level agents of project `project_id` that `order`
    /// names (by hex id) in that order, in the places they held between
    /// them. The agents each spawned move with it.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn reorder_agents(
        &mut self,
        project_id: &str,
        order: &[String],
    ) -> Result<(), AgentRegistryError> {
        let slots: Vec<usize> = (0..self.file.agents.len())
            .filter(|&i| {
                let agent = &self.file.agents[i];
                agent.project_id.as_deref() == Some(project_id)
                    && self.is_top_level(agent)
                    && order.contains(&agent.agent_id().to_hex())
            })
            .collect();
        if reorder_slots(&mut self.file.agents, &slots, order, |a| {
            a.agent_id().to_hex()
        }) {
            self.persist()?;
        }
        Ok(())
    }
}

/// Put the entries `order` names into the slots `slots` of `items` that
/// hold them, in `order`'s order. Entries it leaves out keep their
/// relative order after the ones it names; other slots are untouched.
fn reorder_slots<T: Clone>(
    items: &mut [T],
    slots: &[usize],
    order: &[String],
    key: impl Fn(&T) -> String,
) -> bool {
    let held: Vec<String> = slots.iter().map(|&slot| key(&items[slot])).collect();
    let mut wanted: Vec<String> = Vec::with_capacity(held.len());
    for id in order {
        if held.contains(id) && !wanted.contains(id) {
            wanted.push(id.clone());
        }
    }
    for id in &held {
        if !wanted.contains(id) {
            wanted.push(id.clone());
        }
    }
    if wanted == held {
        return false;
    }
    let taken: Vec<T> = slots.iter().map(|&slot| items[slot].clone()).collect();
    for (&slot, id) in slots.iter().zip(&wanted) {
        if let Some(item) = taken.iter().find(|item| key(item) == *id) {
            items[slot] = item.clone();
        }
    }
    true
}

/// Whether `agent` is project `project_id`'s own: in it and not a channel
/// member, which lives with its channel.
fn owned_by_project(agent: &RegisteredAgent, project_id: &str, members: &HashSet<AgentId>) -> bool {
    agent.project_id.as_deref() == Some(project_id) && !members.contains(&agent.agent_id())
}
