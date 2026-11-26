//! An agent's `send_to_agent`, `list_agents` and `get_agent_state`
//! ([`PeerHooks`]).

use super::*;

/// What `list_agents` and `get_agent_state` say about `peer`.
pub(super) fn peer_row(hub: &AgentHub, peer: &Peer) -> serde_json::Value {
    let (owes, report) = {
        let state = lock(&hub.inbox.state);
        (
            state.owed.get(&peer.agent.agent_id).map_or(0, Vec::len),
            state.reports.get(&peer.agent.agent_id).cloned(),
        )
    };
    let mut row = serde_json::json!({
        "agent_id": peer.agent.agent_id.to_hex(),
        "code": peer.agent.code,
        "name": peer.agent.name,
        "definition": peer.agent.definition,
        "relation": peer.relation.as_str(),
        "working": hub.is_running(peer.agent.agent_id),
        "messages_to_answer": owes,
    });
    if let Some((finished, text)) = report {
        row["last_report"] = serde_json::json!({ "finished": finished, "text": text });
    }
    row
}

fn unknown_agent(target: &str) -> String {
    format!("unknown agent '{target}'; call list_agents to see the agents you can reach")
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
        Some(send(&hub, &circle, InboxKind::Request, peer, subject, text, None).map(|_| ()))
    }

    fn host(&self, target: &str) -> Result<&AgentHooks, String> {
        self.host.as_deref().ok_or_else(|| unknown_agent(target))
    }
}

#[async_trait]
impl AgentControlHook for PeerHooks {
    async fn deliver_message(
        &self,
        target_agent_id: &str,
        parent_agent_id: Option<&str>,
        originating_user_id: Option<&str>,
        project_id: Option<&str>,
        content: &str,
        attachments: Option<serde_json::Value>,
        model: Option<&str>,
    ) -> Result<(), String> {
        self.deliver_message_in_thread(
            target_agent_id,
            parent_agent_id,
            originating_user_id,
            project_id,
            content,
            attachments,
            model,
            None,
        )
        .await
    }

    async fn deliver_message_in_thread(
        &self,
        target_agent_id: &str,
        parent_agent_id: Option<&str>,
        originating_user_id: Option<&str>,
        project_id: Option<&str>,
        content: &str,
        attachments: Option<serde_json::Value>,
        model: Option<&str>,
        subject: Option<&str>,
    ) -> Result<(), String> {
        if let Some(sent) = self.send_to_peer(target_agent_id, content, subject).await {
            return sent;
        }
        self.host(target_agent_id)?
            .deliver_message_in_thread(
                target_agent_id,
                parent_agent_id,
                originating_user_id,
                project_id,
                content,
                attachments,
                model,
                subject,
            )
            .await
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
