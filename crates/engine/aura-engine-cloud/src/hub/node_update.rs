//! Bringing an old node up to the hub's aura without cutting off a turn
//! ([`CloudHub::update_outdated_node`]).
//!
//! ## Invariants
//!
//! - One update of an agent's node at a time: a second call waits for
//!   the first, then finds the node current.
//! - No restart while a turn may run there:
//!   - one this hub or an earlier one asked for (its mark, kept until
//!     the run is known to have ended);
//!   - work this hub counts on the machine (the fleet's busy count,
//!     checked and taken under one lock, [`crate::fleet::VmFleet::update_node`]);
//!   - anything the node itself runs, a turn it started for mail
//!     included: the node is asked (`/v1/cloud/processes`), and a node
//!     too old to answer counts as idle only once its hub socket has
//!     reported what it runs.
//! - When any of that can't be told, the node stays as it is until a
//!   later turn.

use std::sync::Arc;

use aura_core_protocol::cloud::SHIP_TOOL;

use super::CloudHub;
use crate::boot::BootReporter;
use crate::fleet::NodeUpdate;

/// Why a node runs something, or can't be told not to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NodeActivity {
    /// Nothing runs there.
    Idle,
    /// A turn or a spawned agent runs there.
    Running,
    /// The node couldn't say.
    Unknown(String),
}

impl CloudHub {
    /// Update `agent`'s node when it lacks `git_ship`, nothing runs on
    /// it, and the hub has a cloud build to give it. Returns whether the
    /// node was restarted. A failure is logged and shown, and the old
    /// node keeps working.
    pub(super) async fn update_outdated_node(&self, agent: &str, boot: &BootReporter) -> bool {
        let lock = Arc::clone(self.updates.lock().entry(agent.to_string()).or_default());
        let _one_at_a_time = lock.lock().await;
        if self.fleet.node_offers(agent, SHIP_TOOL) {
            return false;
        }
        if self.turn_marked(agent) {
            tracing::info!(%agent, "not updating an old cloud node: a turn is marked under way there");
            return false;
        }
        match self.node_activity(agent).await {
            NodeActivity::Idle => {}
            NodeActivity::Running => {
                tracing::info!(%agent, "not updating an old cloud node: it runs a turn of its own");
                return false;
            }
            NodeActivity::Unknown(why) => {
                tracing::info!(%agent, %why, "not updating an old cloud node: can't tell whether it runs a turn");
                return false;
            }
        }
        self.install_build(agent, boot).await
    }

    /// What `agent`'s node runs: by its own answer, else, for a node too
    /// old to give one, by what its hub socket last reported.
    pub(super) async fn node_activity(&self, agent: &str) -> NodeActivity {
        let node = match self.node(agent) {
            Ok(node) => node,
            Err(err) => return NodeActivity::Unknown(err.to_string()),
        };
        match node.processes().await {
            Ok(Some(rows)) if rows.iter().any(|row| row.is_running()) => NodeActivity::Running,
            Ok(Some(_)) => NodeActivity::Idle,
            Ok(None) => self.reported_activity(agent),
            Err(err) => NodeActivity::Unknown(err.to_string()),
        }
    }

    /// What `agent`'s node last reported on its hub socket: unknown
    /// until the socket said anything since it opened.
    fn reported_activity(&self, agent: &str) -> NodeActivity {
        match self.reported_running(agent) {
            Some(true) => NodeActivity::Running,
            Some(false) => NodeActivity::Idle,
            None => NodeActivity::Unknown("its hub socket hasn't reported what runs there".into()),
        }
    }

    /// Put the hub's cloud build on `agent`'s machine and restart its
    /// node, as [`crate::fleet::VmFleet::update_node`] does.
    async fn install_build(&self, agent: &str, boot: &BootReporter) -> bool {
        let binary = match self.snapshots.local_binary().await {
            Ok(Some(binary)) => binary,
            Ok(None) => return false,
            Err(err) => {
                tracing::warn!(%agent, error = %err, "no aura build to update an old cloud node with");
                return false;
            }
        };
        if let Err(err) = binary.ensure_loads_on_machines(&self.home) {
            tracing::warn!(%agent, error = %err, "the hub's aura build can't run on the machines");
            return false;
        }
        match self.fleet.update_node(agent, binary.bytes, boot).await {
            Ok(update) => update == NodeUpdate::Updated,
            Err(err) => {
                tracing::warn!(%agent, error = %err, "could not update an old cloud node");
                boot.fact("node", format!("update failed, kept the old node: {err}"));
                false
            }
        }
    }
}
