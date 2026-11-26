use super::*;

/// The `<channel>` block for the turns of `agent_id` when it coordinates
/// a channel: its slug, title, description, project and members.
/// `None` for any other agent, or when the channel cannot be read (logged).
///
/// Reads the channel store synchronously: a handful of point reads and
/// short prefix scans of one channel, cheap enough for the turn setup it
/// runs in, which already reads definition files.
pub(crate) fn channel_prompt(hub: &AgentHub, agent_id: AgentId) -> Option<String> {
    let store = hub.channels.as_ref()?;
    let read = || -> Result<_, aura_store_db::ChannelError> {
        let Some(channel_id) = store.channel_of(&agent_id)? else {
            return Ok(None);
        };
        let Some(channel) = store.get(&channel_id)? else {
            return Ok(None);
        };
        if channel.coordinator != agent_id {
            return Ok(None);
        }
        let members = store.members(&channel.id)?;
        Ok(Some((channel, members)))
    };
    match read() {
        Ok(found) => {
            let (channel, members) = found?;
            let registry = lock(&hub.registry);
            Some(render(&registry, &channel, &members))
        }
        Err(error) => {
            warn!(
                agent = %agent_id.to_hex(),
                error = %error,
                "Could not read the channel for the coordinator's prompt"
            );
            None
        }
    }
}

/// Whether `definition` is the channel coordinator's.
pub(crate) fn is_coordinator(definition: Option<&str>) -> bool {
    definition == Some(COORDINATOR_DEFINITION)
}

fn render(registry: &AgentRegistry, channel: &ChannelRecord, members: &[MemberRecord]) -> String {
    let mut out = String::from("<channel>\n");
    let _ = writeln!(
        out,
        "You coordinate the channel #{}: {}",
        channel.slug, channel.title
    );
    if !channel.description.is_empty() {
        let _ = writeln!(out, "Description: {}", channel.description);
    }
    render_project(&mut out, registry, channel);
    out.push_str("Members:\n");
    for member in members {
        render_member(&mut out, registry, member);
    }
    out.push_str("</channel>");
    out
}

/// The `Project:` line of [`render`].
fn render_project(out: &mut String, registry: &AgentRegistry, channel: &ChannelRecord) {
    match &channel.project_id {
        Some(project_id) if is_known_project(registry, project_id) => {
            let view = project_view(registry, project_id);
            let _ = writeln!(
                out,
                "Project: {} ({}). Agents you spawn work there.",
                view.name, view.folder
            );
        }
        Some(_) => out.push_str(
            "Project: missing (unknown or removed). Agents you spawn work where you do; \
             ask the user to pick another project.\n",
        ),
        None => out.push_str("Project: none. Agents you spawn work in the channel's folder.\n"),
    }
}

/// The line of [`render`] listing `member`.
fn render_member(out: &mut String, registry: &AgentRegistry, member: &MemberRecord) {
    {
        let agent = registry.find(member.agent_id);
        let code = agent
            .and_then(|a| a.code.as_deref())
            .unwrap_or("?")
            .to_string();
        let name = agent.map_or_else(
            || member.agent_id.to_hex(),
            |a| a.display_name().into_owned(),
        );
        let role = match member.role {
            MemberRole::Coordinator => "coordinator",
            MemberRole::Agent => "agent",
            MemberRole::Subagent => "subagent",
        };
        let _ = write!(out, "- {code} {name} ({role})");
        if let Some(parent) = member.parent {
            let parent_code = registry
                .find(parent)
                .and_then(|a| a.code.as_deref())
                .unwrap_or("?");
            let _ = write!(out, ", brought in by {parent_code}");
        }
        out.push('\n');
    }
}
