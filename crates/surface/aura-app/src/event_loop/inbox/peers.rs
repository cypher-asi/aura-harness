//! Who an agent is, the channel it is in and the peers it can reach.

use super::*;

/// An agent as the inbox names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Named {
    pub agent_id: AgentId,
    pub code: Option<String>,
    pub name: String,
    pub definition: Option<String>,
}

impl Named {
    pub(super) fn of(agent: &RegisteredAgent) -> Self {
        Self {
            agent_id: agent.agent_id(),
            code: agent.code.clone(),
            name: agent.name.clone(),
            definition: agent.definition.clone(),
        }
    }

    /// Its code, else its name.
    pub(super) fn short(&self) -> &str {
        self.code.as_deref().unwrap_or(&self.name)
    }

    /// `REN4 (rust-engineer #4)`, or its name when it has no code.
    pub(super) fn full(&self) -> String {
        match &self.code {
            Some(code) => format!("{code} ({})", self.name),
            None => self.name.clone(),
        }
    }

    /// Whether it answers requests on its own: every agent but a
    /// coordinator, whose words go to its channel.
    pub(super) fn answers(&self) -> bool {
        self.definition.as_deref() != Some(COORDINATOR_DEFINITION)
    }
}

/// How a peer is related to the agent reaching it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Relation {
    /// The coordinator of the agent's channel
    Coordinator,
    /// Another member of the agent's channel
    Member,
    /// A top-level agent of the agent's project
    Project,
}

impl Relation {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Coordinator => "coordinator",
            Self::Member => "member",
            Self::Project => "project",
        }
    }
}

/// An agent another can reach through its inbox.
#[derive(Debug, Clone)]
pub(crate) struct Peer {
    pub agent: Named,
    pub relation: Relation,
}

/// Who an agent is, the channel it is in and the peers it can reach.
#[derive(Debug, Clone)]
pub(crate) struct Circle {
    pub me: Named,
    /// The slug of the agent's channel, and whether it coordinates it
    pub channel: Option<(String, bool)>,
    pub peers: Vec<Peer>,
}

impl Circle {
    /// Whether the agent coordinates its channel.
    pub(crate) fn coordinates(&self) -> bool {
        matches!(self.channel, Some((_, true)))
    }

    /// The peer `target` names: by code, hex id or name (any case), with
    /// `coordinator` naming the channel's coordinator.
    pub(crate) fn find(&self, target: &str) -> Option<&Peer> {
        let target = target.trim().trim_start_matches('@');
        if target.eq_ignore_ascii_case("coordinator") {
            return self.coordinator();
        }
        let hex = target.to_ascii_lowercase();
        self.peers.iter().find(|peer| {
            peer.agent
                .code
                .as_deref()
                .is_some_and(|code| code.eq_ignore_ascii_case(target))
                || peer.agent.agent_id.to_hex() == hex
                || peer.agent.name.eq_ignore_ascii_case(target)
        })
    }

    pub(super) fn coordinator(&self) -> Option<&Peer> {
        self.peers
            .iter()
            .find(|peer| peer.relation == Relation::Coordinator)
    }

    pub(super) fn slug(&self) -> &str {
        self.channel.as_ref().map_or("", |(slug, _)| slug.as_str())
    }
}

/// The circle of `caller`; `None` for an agent the registry doesn't have,
/// a channel member's subagent, or an agent in no channel that another
/// spawned. A channel's own agents always have one, however they joined
/// ([`channels::is_channel_agent`]): the channel decides, not the spawn
/// tree. Reads the channel store synchronously: on the async runtime use
/// [`circle`].
pub(crate) fn circle_of(hub: &AgentHub, caller: AgentId) -> Option<Circle> {
    let channel = read_channel(hub, caller);
    let registry = lock(&hub.registry);
    let me = registry.find(caller)?;
    let in_channel = channel.as_ref().is_some_and(|(channel, members)| {
        channel.coordinator == caller
            || members.iter().any(|member| {
                member.agent_id == caller && channels::is_channel_agent(&registry, member)
            })
    });
    // A member's subagent is no channel agent: it reaches what its own
    // tree does, as before.
    let channel = channel.filter(|_| in_channel);
    if !in_channel && !registry.is_top_level_agent(caller) {
        return None;
    }
    let hidden = lock(&hub.hidden_members);
    let mut peers: Vec<Peer> = Vec::new();
    let mut slot = None;
    if let Some((channel, members)) = &channel {
        let coordinates = channel.coordinator == caller;
        slot = Some((channel.slug.clone(), coordinates));
        peers = channel_peers(&registry, caller, channel, members);
        if coordinates {
            return Some(Circle {
                me: Named::of(me),
                channel: slot,
                peers,
            });
        }
    }
    if let Some(project) = me
        .project_id
        .as_deref()
        .filter(|_| !hidden.contains(&caller))
    {
        add_project_peers(&registry, &hidden, caller, project, &mut peers);
    }
    Some(Circle {
        me: Named::of(me),
        channel: slot,
        peers,
    })
}

/// The channel `agent` is in, if it isn't archived, with its members;
/// `None` when it can't be read (logged).
fn read_channel(hub: &AgentHub, agent: AgentId) -> Option<(ChannelRecord, Vec<MemberRecord>)> {
    if !lock(&hub.channel_members).contains(&agent) {
        return None;
    }
    let store = hub.channels.as_ref()?;
    let read = || -> Result<_, aura_store_db::ChannelError> {
        let Some(channel_id) = store.channel_of(&agent)? else {
            return Ok(None);
        };
        let Some(channel) = store.get(&channel_id)?.filter(|c| !c.archived) else {
            return Ok(None);
        };
        let members = store.members(&channel.id)?;
        Ok(Some((channel, members)))
    };
    read().unwrap_or_else(|error| {
        warn!(agent = %agent.to_hex(), %error, "Could not read an agent's channel for its peers");
        None
    })
}

/// The `members` of `channel` that `caller` reaches: every one of the
/// channel's own agents ([`channels::is_channel_agent`]) but itself and
/// the archived, wherever in the spawn tree they were registered.
fn channel_peers(
    registry: &AgentRegistry,
    caller: AgentId,
    channel: &ChannelRecord,
    members: &[MemberRecord],
) -> Vec<Peer> {
    let mut peers = Vec::new();
    for member in members {
        if member.agent_id == caller || !channels::is_channel_agent(registry, member) {
            continue;
        }
        let Some(agent) = registry
            .find(member.agent_id)
            .filter(|agent| !agent.archived)
        else {
            continue;
        };
        let relation = if member.agent_id == channel.coordinator {
            Relation::Coordinator
        } else {
            Relation::Member
        };
        peers.push(Peer {
            agent: Named::of(agent),
            relation,
        });
    }
    peers
}

/// Add to `peers` the top-level agents of `project` that `caller` reaches
/// and that aren't among them yet: none that is archived, removed, a
/// coordinator, a compare run or arm, or `hidden`.
fn add_project_peers(
    registry: &AgentRegistry,
    hidden: &HashSet<AgentId>,
    caller: AgentId,
    project: &str,
    peers: &mut Vec<Peer>,
) {
    for agent in registry.agents() {
        let id = agent.agent_id();
        let reachable = id != caller
            && !agent.archived
            && !agent.removed
            && agent.project_id.as_deref() == Some(project)
            && agent.definition.as_deref() != Some(COORDINATOR_DEFINITION)
            && !agent.is_compare()
            && !agent.is_compare_arm()
            && !hidden.contains(&id)
            && registry.is_top_level_agent(id)
            && !peers.iter().any(|peer| peer.agent.agent_id == id);
        if reachable {
            peers.push(Peer {
                agent: Named::of(agent),
                relation: Relation::Project,
            });
        }
    }
}

/// [`circle_of`] off the async runtime.
pub(crate) async fn circle(hub: &Arc<AgentHub>, caller: AgentId) -> Option<Circle> {
    let hub = Arc::clone(hub);
    match tokio::task::spawn_blocking(move || circle_of(&hub, caller)).await {
        Ok(circle) => circle,
        Err(error) => {
            warn!(%error, "Could not look up an agent's peers");
            None
        }
    }
}
