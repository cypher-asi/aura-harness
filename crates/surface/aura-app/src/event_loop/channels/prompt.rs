use super::*;

/// How a coordinator works and talks, in its `<channel>` block.
const COORDINATOR_RULES: &str = "\
How you work:
- You don't do the work, and you can't read, run or edit anything. To find something out, assign it.
- Hand out every piece of work with `assign`: to a member who fits and is free (`to`), or to a new \
member started from a permanent agent (`agent`) when none does.
- Brief the goal, the context and what done looks like, never how to do it. How a member works, \
subagents included, is its own call: don't tell it which to use or avoid.
- Members report back through your inbox. Check on them with `list_agents` or `get_agent_state`, \
and ask or answer them with `send_to_agent`.
- What you write goes to the channel: results, blockers and decisions the user has to make, \
nothing else.
How you talk:
- Plain words, at most three short paragraphs of a sentence or two, with a blank line between \
them. Lead with the result, the blocker or the decision.
- Name agents by their codes. No file lists, logs, diffs or tool output unless asked.
";

/// The `<channel>` block for the turns of `agent_id` when it is in a
/// channel: for its coordinator, the channel, its members and how a
/// coordinator works and talks; for a top-level member, the channel and
/// how members work together. `None` for a subagent, an agent in no
/// channel, or when the channel cannot be read (logged).
///
/// Reads the channel store synchronously: a handful of point reads and
/// short prefix scans of one channel, cheap enough for the turn setup it
/// runs in, which already reads definition files.
pub(crate) fn channel_prompt(hub: &AgentHub, agent_id: AgentId) -> Option<String> {
    if !lock(&hub.channel_members).contains(&agent_id) {
        return None;
    }
    let store = hub.channels.as_ref()?;
    let read = || -> Result<_, aura_store_db::ChannelError> {
        let Some(channel_id) = store.channel_of(&agent_id)? else {
            return Ok(None);
        };
        let Some(channel) = store.get(&channel_id)? else {
            return Ok(None);
        };
        let members = store.members(&channel.id)?;
        Ok(Some((channel, members)))
    };
    match read() {
        Ok(found) => {
            let (channel, members) = found?;
            let working: HashSet<AgentId> = members
                .iter()
                .map(|member| member.agent_id)
                .filter(|agent| hub.is_working(*agent))
                .collect();
            let registry = lock(&hub.registry);
            if channel.coordinator == agent_id {
                Some(render(&registry, &channel, &members, &working))
            } else if registry.find(agent_id).is_some() && registry.is_top_level_agent(agent_id) {
                Some(render_member(&registry, agent_id, &channel, &members))
            } else {
                None
            }
        }
        Err(error) => {
            warn!(
                agent = %agent_id.to_hex(),
                error = %error,
                "Could not read the channel for an agent's prompt"
            );
            None
        }
    }
}

/// Whether `definition` is the channel coordinator's.
pub(crate) fn is_coordinator(definition: Option<&str>) -> bool {
    definition == Some(COORDINATOR_DEFINITION)
}

/// The coordinator's `<channel>` block. Its members are the top-level
/// ones, each said to be `working` when in `working`, else `free`, with
/// the title of what it is (or last was) on; their subagents are only
/// counted.
fn render(
    registry: &AgentRegistry,
    channel: &ChannelRecord,
    members: &[MemberRecord],
    working: &HashSet<AgentId>,
) -> String {
    let mut out = String::from("<channel>\n");
    let _ = writeln!(
        out,
        "You coordinate the channel #{}: {}",
        channel.slug, channel.title
    );
    write_about(&mut out, registry, channel);
    out.push_str("Members:\n");
    for member in top_level_members(registry, members) {
        let _ = write!(out, "- {}", label(registry, member.agent_id));
        if member.agent_id == channel.coordinator {
            out.push_str(" (you)\n");
            continue;
        }
        let busy = working.contains(&member.agent_id);
        out.push_str(if busy { ": working" } else { ": free" });
        if let Some(title) = registry
            .find(member.agent_id)
            .and_then(|agent| agent.title.as_deref())
        {
            let on = if busy { " on" } else { ", last on" };
            let _ = write!(out, "{on} \"{title}\"");
        }
        let subagents = members
            .iter()
            .filter(|m| m.role == MemberRole::Subagent && m.parent == Some(member.agent_id))
            .count();
        if subagents > 0 {
            let _ = write!(out, ", {subagents} subagent(s)");
        }
        out.push('\n');
    }
    out.push_str(COORDINATOR_RULES);
    out.push_str("</channel>");
    out
}

/// The `<channel>` block of `agent_id`, a top-level member of `channel`
/// but not its coordinator.
fn render_member(
    registry: &AgentRegistry,
    agent_id: AgentId,
    channel: &ChannelRecord,
    members: &[MemberRecord],
) -> String {
    let coordinator = code_or_name(registry, channel.coordinator);
    let mut out = String::from("<channel>\n");
    let _ = writeln!(
        out,
        "You are {}, a member of the channel #{}: {}, which {coordinator} coordinates.",
        label(registry, agent_id),
        channel.slug,
        channel.title
    );
    write_about(&mut out, registry, channel);
    let others: Vec<String> = top_level_members(registry, members)
        .filter(|member| member.agent_id != agent_id && member.agent_id != channel.coordinator)
        .map(|member| label(registry, member.agent_id))
        .collect();
    if !others.is_empty() {
        let _ = writeln!(out, "Other members: {}", others.join(", "));
    }
    let _ = write!(
        out,
        "- How you do your work is your call, including whether to hand parts of it to \
         subagents with `task`.\n\
         - Settle details with the other members directly with `send_to_agent`, by code; \
         their answers come back on their own.\n\
         - Keep {coordinator} to results, blockers and decisions. End each piece of work \
         with a short report: it goes back to whoever asked.\n"
    );
    out.push_str("</channel>\n\n");
    out.push_str(crate::event_loop::factory::FACTORY_RULES);
    out
}

/// The description and project lines of `channel`'s block.
fn write_about(out: &mut String, registry: &AgentRegistry, channel: &ChannelRecord) {
    if !channel.description.is_empty() {
        let _ = writeln!(out, "Description: {}", channel.description);
    }
    match &channel.project_id {
        Some(project_id) if is_known_project(registry, project_id) => {
            let view = project_view(registry, project_id);
            let _ = writeln!(
                out,
                "Project: {} ({}). The channel's agents work there.",
                view.name, view.folder
            );
        }
        Some(_) => out.push_str(
            "Project: missing (unknown or removed). The user needs to pick another project.\n",
        ),
        None => out.push_str("Project: none. The channel's agents work in its own folder.\n"),
    }
}

/// The members of `members` no registered agent spawned: the coordinator
/// and its agents, not their subagents.
fn top_level_members<'m>(
    registry: &'m AgentRegistry,
    members: &'m [MemberRecord],
) -> impl Iterator<Item = &'m MemberRecord> {
    members.iter().filter(|member| {
        member.role != MemberRole::Subagent
            && registry.find(member.agent_id).is_some()
            && registry.is_top_level_agent(member.agent_id)
    })
}

/// `REN4 rust-engineer #4`: an agent's code and name, as the blocks name
/// it.
fn label(registry: &AgentRegistry, agent_id: AgentId) -> String {
    match registry.find(agent_id) {
        Some(agent) => match agent.code.as_deref() {
            Some(code) => format!("{code} {}", agent.name),
            None => agent.name.clone(),
        },
        None => agent_id.to_hex(),
    }
}

/// An agent's code, or its name when it has none.
fn code_or_name(registry: &AgentRegistry, agent_id: AgentId) -> String {
    registry.find(agent_id).map_or_else(
        || agent_id.to_hex(),
        |agent| agent.code.clone().unwrap_or_else(|| agent.name.clone()),
    )
}
