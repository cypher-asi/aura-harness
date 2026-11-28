//! An agent's skill folders, sent to its cloud node before a turn so the
//! node lists them and serves `load_skill` itself; the turn's prompt then
//! carries none of their text ([`CloudHub::deliver_skills`]).
//!
//! # Invariants
//!
//! * A bundle is sent only when its digest differs from the last one
//!   this hub delivered to the agent on the same node. The record is in
//!   memory: a hub started since sends each agent's skills once more.
//! * A node that can't take them (one from before this, answering `404`)
//!   or a failed send leaves the turn with every skill's text, as before.

use aura_core_protocol::cloud::{SkillBundle, SkillsSynced};
use aura_core_types::AgentId;

use super::node_client::NodeClient;
use super::{CloudHub, TurnAsk};

/// What a node last took for an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Sent {
    /// The node's address: a new machine starts without the skills
    node: String,
    digest: String,
}

/// How an agent's skills reached its node for a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillsDelivery {
    /// The turn has no skills to send
    None,
    /// Sent now
    Sent,
    /// The node has them already
    Current,
    /// The node can't take them, or the send failed: the turn carries
    /// their text
    Unsupported,
}

impl CloudHub {
    /// Bring `agent`'s node the skills `ask` carries, when they changed.
    /// When it can't take them, `ask` gets the system prompt with their
    /// text instead.
    pub async fn deliver_skills(
        &self,
        node: &NodeClient,
        agent: AgentId,
        ask: &mut TurnAsk,
    ) -> SkillsDelivery {
        let Some(skills) = ask.skills.take() else {
            return SkillsDelivery::None;
        };
        let hex = agent.to_hex();
        let sent = Sent {
            node: node.base().to_string(),
            digest: skills.bundle.digest.clone(),
        };
        if self.skills_sent.lock().get(&hex) == Some(&sent) {
            return SkillsDelivery::Current;
        }
        match put_skills(node, agent, &skills.bundle).await {
            Ok(Some(synced)) if synced.digest == sent.digest => {
                tracing::info!(agent = %hex, digest = %sent.digest, "Sent the agent's skills to its node");
                self.skills_sent.lock().insert(hex, sent);
                SkillsDelivery::Sent
            }
            outcome => {
                match outcome {
                    Ok(None) => {
                        tracing::info!(agent = %hex, "The node takes no skills; the turn carries them");
                    }
                    Ok(Some(_)) => {
                        tracing::warn!(agent = %hex, "The node took other skills; the turn carries them");
                    }
                    Err(err) => {
                        tracing::warn!(agent = %hex, error = %err, "Could not send the skills; the turn carries them");
                    }
                }
                self.skills_sent.lock().remove(&hex);
                ask.system_prompt = Some(skills.full_system_prompt);
                SkillsDelivery::Unsupported
            }
        }
    }
}

/// `PUT /v1/cloud/skills/:agent_id`: `Ok(None)` from a node without it.
async fn put_skills(
    node: &NodeClient,
    agent: AgentId,
    bundle: &SkillBundle,
) -> crate::CloudResult<Option<SkillsSynced>> {
    let op = "send skills";
    let path = format!("/v1/cloud/skills/{}", agent.to_hex());
    let response = (node.authed(reqwest::Method::PUT, &path).json(bundle))
        .timeout(super::node_client::NODE_TIMEOUT)
        .send()
        .await
        .map_err(|err| node.fail(op, err))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let body: String = body.chars().take(300).collect();
        return Err(node.fail(op, format!("{status}: {body}")));
    }
    response
        .json()
        .await
        .map(Some)
        .map_err(|err| node.fail(op, err))
}
