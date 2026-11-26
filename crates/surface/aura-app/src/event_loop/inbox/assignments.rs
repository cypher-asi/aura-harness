//! `assign`: how a channel's coordinator hands work to the channel's
//! members ([`AssignTool`]).

use super::*;

/// What `assign` is called with.
#[derive(Debug, Deserialize)]
pub(crate) struct AssignArgs {
    /// Code of the member to give the work to
    #[serde(default)]
    pub to: Option<String>,
    /// Permanent agent to start a new member from
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub subject: String,
    pub prompt: String,
    #[serde(default)]
    pub todo_id: Option<String>,
}

/// `assign`: how a channel's coordinator hands work to the channel's
/// members ([`assign`]).
pub struct AssignTool {
    link: HubLink,
}

impl AssignTool {
    #[must_use]
    pub fn new(link: HubLink) -> Self {
        Self { link }
    }
}

#[async_trait]
impl Tool for AssignTool {
    fn name(&self) -> &str {
        ASSIGN_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        // Turns offer the schema listing the current permanent agents.
        self.link
            .hub()
            .and_then(|hub| assign_tool_definition(&lock(&hub.definitions)))
            .unwrap_or_else(|| {
                ToolDefinition::new(
                    ASSIGN_TOOL_NAME,
                    "Hand a piece of the channel's work to a member.",
                    serde_json::json!({"type": "object"}),
                )
            })
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let args: AssignArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("assign: {e}")))?;
        let (Some(caller), Some(hub)) = (ctx.caller_agent_id, self.link.hub()) else {
            return Ok(ToolResult::failure(
                ASSIGN_TOOL_NAME,
                "assign: only a channel's coordinator assigns work",
            ));
        };
        Ok(match assign(&hub, caller, args).await {
            Ok(said) => ToolResult::success(ASSIGN_TOOL_NAME, said),
            Err(said) => ToolResult::failure(ASSIGN_TOOL_NAME, said),
        })
    }
}

/// Hand the work `args` describe from the coordinator `caller` to a member
/// of its channel: the one `to` names, else a new one started from the
/// permanent agent `agent`. Returns what the coordinator is told.
///
/// # Errors
/// What the coordinator is told when nothing was assigned.
pub(crate) async fn assign(
    hub: &Arc<AgentHub>,
    caller: AgentId,
    args: AssignArgs,
) -> Result<String, String> {
    let circle = circle(hub, caller)
        .await
        .filter(Circle::coordinates)
        .ok_or_else(|| "assign: only a channel's coordinator assigns work".to_string())?;
    let brief = args.prompt.trim();
    if brief.is_empty() {
        return Err(
            "assign: the prompt is empty; brief the goal, the context and what done looks like"
                .to_string(),
        );
    }
    let (peer, new) = assignee(hub, caller, &circle, &args).await?;
    let subject = Some(args.subject.as_str()).filter(|subject| !subject.trim().is_empty());
    send(
        hub,
        &circle,
        InboxKind::Assignment,
        &peer,
        subject,
        brief,
        args.todo_id.as_deref(),
    )?;
    let status = status_of(hub, &peer, new);
    Ok(format!(
        "Assigned to {} ({status}). Its report will arrive in your inbox.",
        peer.agent.full()
    ))
}

/// The member of `circle`, the channel the coordinator `caller` runs, that
/// the work `args` describe goes to, and whether it is new: the member
/// `to` names, else one started from the permanent agent `agent`.
///
/// # Errors
/// What the coordinator is told when `args` name no member, or no agent
/// to start one from.
async fn assignee(
    hub: &Arc<AgentHub>,
    caller: AgentId,
    circle: &Circle,
    args: &AssignArgs,
) -> Result<(Peer, bool), String> {
    match (named(args.to.as_deref()), named(args.agent.as_deref())) {
        (Some(to), _) => {
            let peer = circle
                .find(&to)
                .filter(|peer| peer.relation == Relation::Member)
                .cloned()
                .ok_or_else(|| no_member(circle, &to))?;
            Ok((peer, false))
        }
        (None, Some(definition)) => Ok((start_member(hub, caller, &definition).await?, true)),
        (None, None) => Err(
            "assign: name a member in `to`, or a permanent agent in `agent` \
                    to start a new member"
                .to_string(),
        ),
    }
}

/// `field` trimmed, when that leaves anything.
fn named(field: Option<&str>) -> Option<String> {
    field
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// How soon the member `peer`, `new` or not, starts the work assigned.
fn status_of(hub: &AgentHub, peer: &Peer, new: bool) -> &'static str {
    if new {
        "new member"
    } else if hub.is_running(peer.agent.agent_id) {
        "busy: it starts when free"
    } else {
        "starting now"
    }
}

/// What the coordinator of `circle` is told when `to` names no member.
fn no_member(circle: &Circle, to: &str) -> String {
    let members: Vec<String> = circle
        .peers
        .iter()
        .filter(|peer| peer.relation == Relation::Member)
        .map(|peer| peer.agent.full())
        .collect();
    let slug = circle.slug();
    if members.is_empty() {
        format!(
            "assign: #{slug} has no member `{to}`, nor any other yet; name a permanent agent in \
             `agent` to start one"
        )
    } else {
        format!(
            "assign: #{slug} has no member `{to}`. Members: {}. Or name a permanent agent in \
             `agent` to start a new one.",
            members.join(", ")
        )
    }
}

/// Start a new member of the channel `coordinator` runs from the
/// permanent agent `definition`, and show it in the agent lists.
async fn start_member(
    hub: &Arc<AgentHub>,
    coordinator: AgentId,
    definition: &str,
) -> Result<Peer, String> {
    let definition = match lock(&hub.definitions).get(definition) {
        Some(found) if !found.is_coordinator() => found.name.clone(),
        Some(_) => {
            return Err(
                "assign: coordinators only run channels; pick another permanent agent".to_string(),
            )
        }
        None => {
            return Err(format!(
                "assign: there is no permanent agent `{definition}`"
            ))
        }
    };
    let adding = Arc::clone(hub);
    let added =
        tokio::task::spawn_blocking(move || channels::add_agent(&adding, coordinator, &definition))
            .await;
    let agent = match added {
        Ok(Ok(agent)) => agent,
        Ok(Err(error)) => return Err(format!("assign: could not start a new member: {error:#}")),
        Err(error) => return Err(format!("assign: could not start a new member: {error}")),
    };
    hub.send_agent_list().await;
    Ok(Peer {
        agent: Named::of(&agent),
        relation: Relation::Member,
    })
}
