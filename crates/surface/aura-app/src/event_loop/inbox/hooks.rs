//! An agent's `send_to_agent`, `list_agents` and `get_agent_state`
//! ([`PeerHooks`]), and the requests a channel member sends from its
//! cloud machine ([`MachineMail`]).

use super::*;
use crate::cloud::hub::{MailDesk, Reach};
use aura_engine::RemoteMail;
use std::fmt::Write as _;

/// What `list_agents` and `get_agent_state` say about `peer`.
pub(super) fn peer_row(hub: &AgentHub, peer: &Peer) -> serde_json::Value {
    let (owes, report) = {
        let state = lock(&hub.inbox.state);
        (
            state.owed.get(&peer.agent.agent_id).map_or(0, Vec::len),
            state.reports.get(&peer.agent.agent_id).cloned(),
        )
    };
    let working = hub.is_working(peer.agent.agent_id);
    let standing = crate::event_loop::standing::state_of(hub, peer.agent.agent_id);
    let mut row = serde_json::json!({
        "agent_id": peer.agent.agent_id.to_hex(),
        "code": peer.agent.code,
        "name": peer.agent.name,
        "definition": peer.agent.definition,
        "relation": peer.relation.as_str(),
        "working": working,
        "state": standing.state,
        "messages_to_answer": owes,
    });
    if let Some(why) = standing.why {
        row["why"] = serde_json::json!(why);
    }
    if let Some((finished, text)) = report {
        row["last_report"] = serde_json::json!({ "finished": finished, "text": text });
    }
    row
}

fn unknown_agent(target: &str) -> String {
    format!("unknown agent '{target}'; call list_agents to see the agents you can reach")
}

/// [`unknown_agent`] for a caller in `circle`, listing the codes of the
/// peers it does reach: its channel's members first.
fn unknown_in(target: &str, circle: &Circle) -> String {
    if circle.peers.is_empty() {
        return unknown_agent(target);
    }
    let codes = |related: fn(Relation) -> bool| -> Vec<&str> {
        circle
            .peers
            .iter()
            .filter(|peer| related(peer.relation))
            .map(|peer| peer.agent.short())
            .collect()
    };
    let members = codes(|r| r != Relation::Project);
    let project = codes(|r| r == Relation::Project);
    let mut said = format!("unknown agent '{target}'");
    if !members.is_empty() {
        let _ = write!(
            said,
            "; the members of #{} are: {}",
            circle.slug(),
            members.join(", ")
        );
    }
    if !project.is_empty() {
        let _ = write!(said, "; in your project: {}", project.join(", "));
    }
    said.push_str(". Address them by code or agent id");
    said
}

/// Whether `error` says its target is unknown, so a better one may
/// replace it.
fn is_unknown(error: &str) -> bool {
    error.starts_with("unknown agent")
}

/// An agent's `send_to_agent`, `list_agents` and `get_agent_state`: they
/// reach its peers through their inboxes, and hand anything else to the
/// background agents' hooks when those run.
pub struct PeerHooks {
    link: HubLink,
    caller: AgentId,
    host: Option<Arc<AgentHooks>>,
}

impl PeerHooks {
    /// The hooks of `caller`'s tools, falling back to `host`.
    #[must_use]
    pub fn new(link: HubLink, caller: AgentId, host: Option<Arc<AgentHooks>>) -> Self {
        Self { link, caller, host }
    }

    async fn circle(&self) -> Option<(Arc<AgentHub>, Circle)> {
        let hub = self.link.hub()?;
        let circle = circle(&hub, self.caller).await?;
        Some((hub, circle))
    }

    /// Send `text` to the peer `target` names, about `subject`. `None`
    /// when `target` names no peer.
    async fn send_to_peer(
        &self,
        target: &str,
        text: &str,
        subject: Option<&str>,
    ) -> Option<Result<(), String>> {
        let (hub, circle) = self.circle().await?;
        let peer = circle.find(target)?;
        let sent = send(&hub, &circle, InboxKind::Request, peer, subject, text, None).map(|_| ());
        // The sender is told it went once it is on disk.
        if sent.is_ok() {
            super::saved::persist(&hub).await;
        }
        Some(sent)
    }

    /// The error for a `target` neither the caller's peers nor its host
    /// know: the peers' codes when it has a circle, else `error` as it is.
    async fn unknown(&self, target: &str, error: String) -> String {
        match self.circle().await {
            Some((_, circle)) if !circle.peers.is_empty() => unknown_in(target, &circle),
            _ => error,
        }
    }

    fn host(&self, target: &str) -> Result<&AgentHooks, String> {
        self.host.as_deref().ok_or_else(|| unknown_agent(target))
    }
}

#[async_trait]
impl AgentControlHook for PeerHooks {
    async fn deliver_message(&self, message: AgentMessage<'_>) -> Result<(), String> {
        self.deliver_message_in_thread(message, None).await
    }

    async fn deliver_message_in_thread(
        &self,
        message: AgentMessage<'_>,
        subject: Option<&str>,
    ) -> Result<(), String> {
        let target_agent_id = message.target_agent_id;
        if let Some(sent) = self
            .send_to_peer(target_agent_id, message.content, subject)
            .await
        {
            return sent;
        }
        let sent = match self.host.as_deref() {
            Some(host) => host.deliver_message_in_thread(message, subject).await,
            None => Err(unknown_agent(target_agent_id)),
        };
        match sent {
            Err(error) if is_unknown(&error) => Err(self.unknown(target_agent_id, error).await),
            other => other,
        }
    }

    async fn lifecycle(
        &self,
        target_agent_id: &str,
        parent_agent_id: Option<&str>,
        originating_user_id: Option<&str>,
        action: &str,
    ) -> Result<(), String> {
        self.host(target_agent_id)?
            .lifecycle(
                target_agent_id,
                parent_agent_id,
                originating_user_id,
                action,
            )
            .await
    }

    async fn delegate_task(
        &self,
        target_agent_id: &str,
        parent_agent_id: Option<&str>,
        originating_user_id: Option<&str>,
        task: &str,
        context: Option<&serde_json::Value>,
        model: Option<&str>,
    ) -> Result<(), String> {
        let text = match context {
            Some(context) => format!("{task}\n\nContext: {context}"),
            None => task.to_string(),
        };
        if let Some(sent) = self.send_to_peer(target_agent_id, &text, None).await {
            return sent;
        }
        self.host(target_agent_id)?
            .delegate_task(
                target_agent_id,
                parent_agent_id,
                originating_user_id,
                task,
                context,
                model,
            )
            .await
    }
}

/// Whom `agent`, a channel member, reaches from its cloud machine: its
/// peers here, by the names they go by here. `None` for an agent in no
/// channel, which mails its machine's project as before.
pub(crate) async fn reach_of(hub: &Arc<AgentHub>, agent: AgentId) -> Option<Vec<Reach>> {
    if !lock(&hub.channel_members).contains(&agent) {
        return None;
    }
    let circle = circle(hub, agent).await?;
    let reach = circle.peers.iter().map(|peer| Reach {
        agent_id: peer.agent.agent_id,
        name: peer.agent.full(),
    });
    Some(reach.collect())
}

/// Bring the reach of every channel member placed on `cloud` up to date
/// ([`reach_of`]), so the requests each sends its peers go through the
/// inbox here, never straight to a peer's machine. A reach is otherwise
/// set only as its member's turn starts, and lives only as long as the
/// hub: a hub started since, or a member who joined meanwhile, would
/// leave its mail outside the inbox.
pub(crate) async fn refresh_reach(hub: &Arc<AgentHub>, cloud: &crate::cloud::CloudHub) {
    let placed: Vec<AgentId> = lock(&hub.channel_members)
        .iter()
        .copied()
        .filter(|member| cloud.is_placed(*member))
        .collect();
    for member in placed {
        if let Some(peers) = reach_of(hub, member).await {
            cloud.set_reach(member, peers);
        }
    }
}

/// The requests a channel member sends from its cloud machine to the
/// agents it reaches ([`reach_of`]), filed as its `send_to_agent` here
/// files them.
pub(crate) struct MachineMail(pub(crate) Weak<AgentHub>);

#[async_trait]
impl MailDesk for MachineMail {
    async fn file(&self, mail: RemoteMail) -> Result<(), String> {
        let hub = self.0.upgrade().ok_or("the terminal is shutting down")?;
        let to = mail.to.to_hex();
        let circle = circle(&hub, mail.from).await;
        let peer = circle
            .as_ref()
            .and_then(|circle| Some((circle, circle.find(&to)?)));
        let Some((circle, peer)) = peer else {
            warn!(from = %mail.from.to_hex(), %to, "A cloud member mailed an agent it doesn't reach");
            return Err(circle
                .as_ref()
                .map_or_else(|| unknown_agent(&to), |circle| unknown_in(&to, circle)));
        };
        let subject = Some(mail.subject.as_str()).filter(|subject| !subject.is_empty());
        let sent = send(
            &hub,
            circle,
            InboxKind::Request,
            peer,
            subject,
            &mail.text,
            None,
        );
        match &sent {
            Ok(_) => {
                hub.inbox
                    .refusals
                    .delivered((mail.from, mail.to), &mail.subject);
                // Its machine is acked once the mail is on disk here.
                super::saved::persist(&hub).await;
            }
            Err(error) => {
                warn!(from = %mail.from.to_hex(), %to, %error, "A cloud member's request was refused");
            }
        }
        sent.map(|_| ())
    }

    /// Its machine drops the ack, so the sender is told here, as a notice
    /// into its running turn or the prompt of its next one: once per
    /// recipient and subject, however often the mail is resent
    /// ([`super::refusals`]).
    fn refused(&self, mail: &RemoteMail, why: &str) {
        let Some(hub) = self.0.upgrade() else {
            return;
        };
        let now = std::time::Instant::now();
        if !hub
            .inbox
            .refusals
            .tell((mail.from, mail.to), &mail.subject, now)
        {
            tracing::debug!(from = %mail.from.to_hex(), to = %mail.to.to_hex(), "A refusal its sender was told already; not told again");
            return;
        }
        let to = lock(&hub.registry)
            .find(mail.to)
            .map_or_else(|| mail.to.to_hex(), |agent| Named::of(agent).full());
        if hub
            .notices
            .send(refusal(mail.from, &to, &mail.subject, why))
            .is_err()
        {
            warn!(from = %mail.from.to_hex(), %to, "A refusal after the event loop ended; dropped");
        }
    }
}

/// The notice telling `from` that its message to `to` about `subject`
/// was not delivered, `why`.
pub(super) fn refusal(from: AgentId, to: &str, subject: &str, why: &str) -> Notice {
    let about = if subject.trim().is_empty() {
        String::new()
    } else {
        format!(" about \"{}\"", subject.trim())
    };
    Notice {
        to: from,
        shown: format!("✉ Not delivered to {to}: {why}"),
        prompt: format!(
            "Your message to {to}{about} was not delivered: {why}. It did not reach them; \
             send it again once that is fixed, or tell whoever is waiting on it."
        ),
        inbox: None,
    }
}

#[async_trait]
impl AgentReadHook for PeerHooks {
    async fn snapshot(&self, target_agent_id: &str) -> Result<serde_json::Value, String> {
        if let Some((hub, circle)) = self.circle().await {
            if let Some(peer) = circle.find(target_agent_id) {
                return Ok(peer_row(&hub, peer));
            }
        }
        self.host(target_agent_id)?.snapshot(target_agent_id).await
    }

    async fn list_agents(&self, org_id: Option<&str>) -> Result<serde_json::Value, String> {
        let mut agents = Vec::new();
        if let Some((hub, circle)) = self.circle().await {
            if self.host.is_none() {
                agents.push(serde_json::json!({
                    "agent_id": circle.me.agent_id.to_hex(),
                    "code": circle.me.code,
                    "name": circle.me.name,
                    "is_you": true,
                }));
            }
            agents.extend(circle.peers.iter().map(|peer| peer_row(&hub, peer)));
        }
        if let Some(host) = &self.host {
            match host.list_agents(org_id).await? {
                serde_json::Value::Array(more) => agents.extend(more),
                other => agents.push(other),
            }
        }
        Ok(serde_json::Value::Array(agents))
    }
}
