//! The parent/child structure and the listed tree of agents.

use aura_core_types::AgentId;
use std::collections::{HashMap, HashSet};

use super::{run_order, AgentRegistry, Project, RegisteredAgent};

impl AgentRegistry {
    fn parent_index(&self, index: usize) -> Option<usize> {
        let parent = self.file.agents[index].parent.as_deref()?;
        self.file
            .agents
            .iter()
            .position(|agent| agent.zns_id == parent)
    }

    /// Every agent below `agent_id`: those it spawned, theirs, and so on.
    #[must_use]
    pub fn descendants(&self, agent_id: AgentId) -> Vec<AgentId> {
        let Some(agent) = self.find(agent_id) else {
            return Vec::new();
        };
        let mut found: Vec<&str> = vec![agent.zns_id.as_str()];
        let mut out = Vec::new();
        let mut i = 0;
        while i < found.len() {
            let parent = found[i];
            for child in self
                .file
                .agents
                .iter()
                .filter(|a| a.parent.as_deref() == Some(parent))
            {
                if !found.contains(&child.zns_id.as_str()) {
                    found.push(&child.zns_id);
                    out.push(child.agent_id());
                }
            }
            i += 1;
        }
        out
    }

    /// `agent_id` and the agents above it, nearest first.
    #[must_use]
    pub fn ancestry(&self, agent_id: AgentId) -> Vec<AgentId> {
        let Some(mut index) = self.position(agent_id) else {
            return Vec::new();
        };
        let mut out = vec![agent_id];
        for _ in 0..self.file.agents.len() {
            let Some(parent) = self.parent_index(index) else {
                break;
            };
            index = parent;
            out.push(self.file.agents[index].agent_id());
        }
        out
    }

    /// The agent at the top of `agent_id`'s tree, which nothing registered
    /// spawned; `agent_id` itself when it is at the top or unknown.
    #[must_use]
    pub fn root(&self, agent_id: AgentId) -> AgentId {
        self.ancestry(agent_id).last().copied().unwrap_or(agent_id)
    }

    /// Whether `agent_id` works under a `/compare`: one of its runs, or a
    /// subtask of one. The agents grouping an arm's runs do no work.
    #[must_use]
    pub fn under_compare(&self, agent_id: AgentId) -> bool {
        !self
            .find(agent_id)
            .is_some_and(RegisteredAgent::is_compare_arm)
            && self
                .ancestry(agent_id)
                .into_iter()
                .skip(1)
                .any(|id| self.find(id).is_some_and(RegisteredAgent::is_compare))
    }

    /// The agents holding comparisons, oldest first.
    #[must_use]
    pub fn compares(&self) -> Vec<&RegisteredAgent> {
        self.file.agents.iter().filter(|a| a.is_compare()).collect()
    }

    /// Whether nothing registered spawned `agent`: it is listed at the top
    /// of its project.
    pub(super) fn is_top_level(&self, agent: &RegisteredAgent) -> bool {
        agent
            .parent
            .as_deref()
            .is_none_or(|parent| !self.file.agents.iter().any(|a| a.zns_id == parent))
    }

    /// Listed agents in display order, paired with how deeply each is
    /// nested: project by project, each top-level agent followed by the
    /// agents it spawned, one level deeper. Archived agents and everything
    /// under them are left out (see [`Self::archived_tree`]).
    #[must_use]
    pub fn tree(&self) -> Vec<(&RegisteredAgent, usize)> {
        let index = self.index();
        let mut walk = Walk::new(self.file.agents.len());
        for project in self.file.projects.iter().filter(|p| !p.removed) {
            for agent in self.file.agents.iter().filter(|a| {
                a.project_id.as_deref() == Some(project.id.as_str()) && index.is_top_level(a)
            }) {
                if !agent.archived && !agent.removed {
                    self.visit(&index, agent, 0, false, &mut walk);
                }
            }
        }
        // Channel members belong to their channel, not their project: they
        // stay listed when it is removed. `visit` skips the ones the
        // projects already listed.
        if !self.channel_members.is_empty() {
            for agent in self.file.agents.iter().filter(|a| {
                self.channel_members.contains(&a.agent_id())
                    && index.is_top_level(a)
                    && !a.archived
                    && !a.removed
            }) {
                self.visit(&index, agent, 0, false, &mut walk);
            }
        }
        walk.out
    }

    /// One pass over the agents: where each ZNS id is, and the agents each
    /// one spawned, in registry order.
    fn index(&self) -> Index<'_> {
        let agents = &self.file.agents;
        let mut by_zns = HashMap::with_capacity(agents.len());
        let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, agent) in agents.iter().enumerate() {
            by_zns.entry(agent.zns_id.as_str()).or_insert(i);
            if let Some(parent) = agent.parent.as_deref() {
                children.entry(parent).or_default().push(i);
            }
        }
        Index {
            agents,
            by_zns,
            children,
        }
    }

    /// The Archive: each archived agent no archived agent spawned, the
    /// most recently archived first, followed by the agents it spawned,
    /// with depth counted from it. Agents archived before the date was
    /// kept come last; ties keep project order, then registry order, so
    /// the Archive does not move as agents in it are used.
    #[must_use]
    pub fn archived_tree(&self) -> Vec<(&RegisteredAgent, usize)> {
        let index = self.index();
        let mut walk = Walk::new(self.file.agents.len());
        let is_head =
            |agent: &&RegisteredAgent| agent.archived && !index.spawners(agent).any(|p| p.archived);
        let mut heads: Vec<&RegisteredAgent> = Vec::new();
        for project in &self.file.projects {
            heads.extend(
                self.file
                    .agents
                    .iter()
                    .filter(|a| a.project_id.as_deref() == Some(project.id.as_str()))
                    .filter(is_head),
            );
        }
        heads.extend(
            self.file
                .agents
                .iter()
                .filter(|a| {
                    a.project_id
                        .as_deref()
                        .is_none_or(|id| self.project(id).is_none())
                })
                .filter(is_head),
        );
        // Stable, so equal dates keep the order above; None sorts lowest,
        // so reversed it comes last.
        heads.sort_by_key(|agent| std::cmp::Reverse(agent.archived_at));
        for agent in heads {
            self.visit(&index, agent, 0, true, &mut walk);
        }
        walk.out
    }

    fn visit<'a>(
        &'a self,
        index: &Index<'a>,
        agent: &'a RegisteredAgent,
        depth: usize,
        with_archived: bool,
        walk: &mut Walk<'a>,
    ) {
        // A parent cycle would loop forever; registries never form one, but
        // a hand-edited file could.
        if walk.out.len() > self.file.agents.len() || !walk.seen.insert(agent.zns_id.as_str()) {
            return;
        }
        walk.out.push((agent, depth));
        let mut spawned: Vec<&RegisteredAgent> = index
            .children
            .get(agent.zns_id.as_str())
            .into_iter()
            .flatten()
            .map(|&i| &self.file.agents[i])
            .filter(|a| with_archived || !(a.archived || a.removed))
            .collect();
        if agent.is_compare() || agent.is_compare_arm() {
            // Runs register as their copies become ready, in no set order.
            spawned.sort_by(|x, y| run_order(&x.name).cmp(&run_order(&y.name)));
        }
        for child in spawned {
            self.visit(index, child, depth + 1, with_archived, walk);
        }
    }

    /// The agents that spawned `agent`, nearest first.
    pub(super) fn spawners(&self, agent: &RegisteredAgent) -> Vec<&RegisteredAgent> {
        let mut out = Vec::new();
        let mut current = agent;
        while let Some(parent) = current
            .parent
            .as_deref()
            .and_then(|zns| self.file.agents.iter().find(|a| a.zns_id == zns))
        {
            if out.len() >= self.file.agents.len() {
                break;
            }
            out.push(parent);
            current = parent;
        }
        out
    }

    /// Whether `agent` is on the list: its project is, and neither it nor
    /// an agent that spawned it is archived or left behind by a removal.
    /// A channel member's project doesn't count: it belongs to its channel.
    pub(super) fn is_listed(&self, agent: &RegisteredAgent) -> bool {
        let project_listed = self.channel_members.contains(&agent.agent_id())
            || agent
                .project_id
                .as_deref()
                .and_then(|id| self.project(id))
                .is_some_and(|p| !p.removed);
        project_listed
            && !agent.archived
            && !agent.removed
            && !self.spawners(agent).iter().any(|p| p.archived || p.removed)
    }

    /// Projects on the list, in order: every project not removed, with
    /// its agents listed or all of them archived.
    #[must_use]
    pub fn listed_projects(&self) -> Vec<&Project> {
        self.file.projects.iter().filter(|p| !p.removed).collect()
    }

    /// How many agents of project `project_id` are listed, and how many
    /// sit in the Archive.
    #[must_use]
    pub fn project_counts(&self, project_id: &str) -> (usize, usize) {
        let count = |tree: Vec<(&RegisteredAgent, usize)>| {
            tree.iter()
                .filter(|(a, _)| a.project_id.as_deref() == Some(project_id))
                .count()
        };
        (count(self.tree()), count(self.archived_tree()))
    }

    /// [`Self::project_counts`] of every project, from one walk of each
    /// list.
    #[must_use]
    pub fn project_counts_all(&self) -> HashMap<String, (usize, usize)> {
        let mut counts: HashMap<String, (usize, usize)> = HashMap::new();
        for (agent, _) in self.tree() {
            if let Some(id) = &agent.project_id {
                counts.entry(id.clone()).or_default().0 += 1;
            }
        }
        for (agent, _) in self.archived_tree() {
            if let Some(id) = &agent.project_id {
                counts.entry(id.clone()).or_default().1 += 1;
            }
        }
        counts
    }

    /// `agent_id` and every agent under it.
    #[must_use]
    pub fn subtree(&self, agent_id: AgentId) -> Vec<AgentId> {
        let Some(agent) = self.find(agent_id) else {
            return Vec::new();
        };
        let index = self.index();
        let mut walk = Walk::new(self.file.agents.len());
        self.visit(&index, agent, 0, true, &mut walk);
        walk.out.into_iter().map(|(a, _)| a.agent_id()).collect()
    }
}

/// Lookups built in one pass over the registry's agents, so a walk of the
/// lists finds parents and children without scanning every agent.
struct Index<'a> {
    agents: &'a [RegisteredAgent],
    /// Position of the first agent with each ZNS id
    by_zns: HashMap<&'a str, usize>,
    /// Positions of the agents naming each ZNS id as parent, in order
    children: HashMap<&'a str, Vec<usize>>,
}

impl<'a> Index<'a> {
    fn parent_of(&self, agent: &RegisteredAgent) -> Option<&'a RegisteredAgent> {
        let parent = agent.parent.as_deref()?;
        self.by_zns.get(parent).map(|&i| &self.agents[i])
    }

    /// As [`AgentRegistry::is_top_level`].
    fn is_top_level(&self, agent: &RegisteredAgent) -> bool {
        self.parent_of(agent).is_none()
    }

    /// The agents that spawned `agent`, nearest first. Stops after as many
    /// steps as there are agents, in case of a parent cycle.
    fn spawners<'s>(
        &'s self,
        agent: &'a RegisteredAgent,
    ) -> impl Iterator<Item = &'a RegisteredAgent> + 's
    where
        'a: 's,
    {
        let mut current = agent;
        let mut left = self.agents.len();
        std::iter::from_fn(move || {
            if left == 0 {
                return None;
            }
            left -= 1;
            let parent = self.parent_of(current)?;
            current = parent;
            Some(parent)
        })
    }
}

/// Rows of one walk of the lists, and the agents it has listed.
struct Walk<'a> {
    out: Vec<(&'a RegisteredAgent, usize)>,
    seen: HashSet<&'a str>,
}

impl Walk<'_> {
    fn new(capacity: usize) -> Self {
        Self {
            out: Vec::with_capacity(capacity),
            seen: HashSet::with_capacity(capacity),
        }
    }
}
