//! A placed agent's conversation on its node: handing it the local session
//! at placement and starting a new one there.

use aura_core_types::AgentId;

use crate::{BootReporter, BootStage, CloudResult};

use super::node_client::{NodeClient, NODE_TIMEOUT};
use super::CloudHub;

impl CloudHub {
    /// Give `node` (machine `machine`) `agent`'s current local session, so
    /// its first run there continues the conversation, tool calls
    /// included. Without one, or when the node refuses it, the node
    /// rebuilds the conversation from the record it was given.
    pub(super) async fn upload_session(
        &self,
        node: &NodeClient,
        agent: AgentId,
        head: u64,
        machine: &str,
        boot: &BootReporter,
    ) {
        let Some(sessions) = self.sessions.lock().clone() else {
            return;
        };
        let checkpoint = tokio::task::spawn_blocking(move || sessions.load_current(agent))
            .await
            .ok()
            .flatten()
            .filter(|checkpoint| checkpoint.record_seq <= head);
        let Some(checkpoint) = checkpoint else {
            boot.fact("conversation", "rebuilt from the record");
            return;
        };
        let saved = boot
            .wait(
                BootStage::ImportRecord,
                format!("{machine} is taking the conversation"),
                Some(NODE_TIMEOUT),
                node.put_session(agent, &checkpoint),
            )
            .await;
        match saved {
            Ok(saved) => boot.fact(
                "conversation",
                format!("{} message(s) carried over", saved.messages),
            ),
            Err(err) => {
                tracing::warn!(agent = %agent.to_hex(), error = %err, "could not hand the node the agent's session; it rebuilds it from the record");
                boot.log(
                    BootStage::ImportRecord,
                    true,
                    format!("the conversation is rebuilt from the record: {err}"),
                );
            }
        }
    }

    /// Start a new session for `agent` on its node (`/new`), waking the
    /// machine if it sleeps, and copy the `SessionStart` here. Returns the
    /// agent's head.
    ///
    /// # Errors
    ///
    /// When the machine can't be woken, the node refuses, or the copy
    /// fails.
    pub async fn new_session(&self, agent: AgentId) -> CloudResult<u64> {
        let hex = agent.to_hex();
        let node = self.awake_node(&hex, &BootReporter::none()).await?;
        let _work = self.fleet.begin_work(&hex);
        let started = node.new_session(agent).await?;
        self.mirror_from(&node, agent).await?;
        Ok(started.head_seq)
    }
}
