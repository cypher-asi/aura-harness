//! Bringing an existing machine's node up to the hub's aura.
//!
//! A machine runs the aura its snapshot had when it was made. One made
//! before the merge queue has no `git_ship` and puts no queue marker in
//! its pushes, so its agents can't ship through the queue. When such a
//! node is about to start a turn, and nothing runs on it, the hub uploads
//! its own cloud build, restarts the node and claims it again
//! ([`VmFleet::update_node`]). The node keeps its store on disk, so the
//! agent's conversation survives the restart.
//!
//! ## Invariants
//!
//! - Never while the machine does work this hub counts (`busy > 0`),
//!   and the check and the update's own busy count are taken under one
//!   lock, so nothing this hub starts can begin meanwhile. The update
//!   counts as work on the machine until it is done.
//! - Work the hub doesn't count (a turn the node started itself, mail
//!   for an idle agent) is the caller's to rule out first, by asking the
//!   node (`CloudHub::current_node`): the fleet can't see it.
//! - The new binary is installed only once it starts on the machine
//!   (`--help`); the old one stays in place otherwise.
//! - A failed update leaves the old node running. Its agents get the
//!   fallback ship prompt, and the hub queues their pushes itself.

use std::sync::Arc;
use std::time::Duration;

use super::claim::Accept;
use super::VmFleet;
use crate::boot::BootReporter;
use crate::error::{CloudError, CloudResult};
use crate::snapshots::{START_NODE, STOP_NODE};
use aura_core_protocol::cloud::SHIP_TOOL;

/// Where the new binary waits on the machine before it is installed.
pub(super) const STAGED_BINARY: &str = "/tmp/aura.update";
/// Installs the staged binary once it starts on the machine.
pub const INSTALL_BINARY: &str = "set -e; chmod 0755 /tmp/aura.update; \
/tmp/aura.update --help >/dev/null; \
sudo install -m 0755 /tmp/aura.update /usr/local/bin/aura; rm -f /tmp/aura.update";
/// Limit on installing and restarting the node.
const UPDATE_STEP_TIMEOUT: Duration = Duration::from_secs(180);

/// What [`VmFleet::update_node`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeUpdate {
    /// The node offers `git_ship` already.
    Current,
    /// The machine works right now; the next turn tries again.
    Busy,
    /// The node runs the hub's build now.
    Updated,
}

impl VmFleet {
    /// Whether `agent`'s node offered `tool` at its last claim.
    #[must_use]
    pub fn node_offers(&self, agent: &str, tool: &str) -> bool {
        self.machines
            .lock()
            .get(agent)
            .is_some_and(|entry| entry.node_tools.iter().any(|offered| offered == tool))
    }

    /// Whether `agent`'s node was claimed and lacks `git_ship`.
    fn needs_update(&self, agent: &str) -> bool {
        let machines = self.machines.lock();
        machines.get(agent).is_some_and(|entry| {
            entry.node_token.is_some() && !entry.node_tools.iter().any(|tool| tool == SHIP_TOOL)
        })
    }

    /// Count the update as work on `agent`'s machine, unless other work
    /// runs there: the check and the count under one lock.
    fn begin_update(self: &Arc<Self>, agent: &str) -> Option<super::WorkGuard> {
        let idle = self
            .machines
            .lock()
            .get_mut(agent)
            .filter(|entry| entry.busy == 0)
            .map(|entry| entry.busy += 1)
            .is_some();
        idle.then(|| super::WorkGuard {
            fleet: Arc::clone(self),
            agent: agent.to_string(),
        })
    }

    /// Put `binary`, the hub's cloud build, on `agent`'s machine and
    /// restart its node on it, when its node lacks `git_ship` and the
    /// machine is idle.
    ///
    /// # Errors
    ///
    /// When uploading, installing, restarting or claiming fails; the old
    /// node is left as it was where the install didn't happen.
    pub async fn update_node(
        self: &Arc<Self>,
        agent: &str,
        binary: Vec<u8>,
        boot: &BootReporter,
    ) -> CloudResult<NodeUpdate> {
        if !self.needs_update(agent) {
            return Ok(NodeUpdate::Current);
        }
        // Held until the update is done: nothing this hub starts there
        // meanwhile, and the machine doesn't sleep under it.
        let Some(_updating) = self.begin_update(agent) else {
            return Ok(NodeUpdate::Busy);
        };
        let machine = self
            .view(agent)
            .ok_or_else(|| CloudError::NotFound {
                what: "machine",
                name: agent.to_string(),
            })?
            .machine
            .name;
        boot.fact("node", "updating to the hub's aura (git_ship, merge queue)");
        let provider = self.provider();
        provider.upload(&machine, STAGED_BINARY, binary).await?;
        for (what, command) in [
            ("install the new aura", INSTALL_BINARY.to_string()),
            ("restart the node", format!("{STOP_NODE}; {START_NODE}")),
        ] {
            let out = provider
                .exec(&machine, &command, None, UPDATE_STEP_TIMEOUT)
                .await?;
            if !out.success() {
                return Err(CloudError::ExecFailed {
                    machine: machine.clone(),
                    command: what.to_string(),
                    exit_code: out.exit_code,
                    stderr: out.stderr_tail(400),
                });
            }
        }
        self.claim_reported(agent, &machine, &[], Accept::Any, boot)
            .await?;
        tracing::info!(%agent, %machine, "cloud node updated to the hub's aura");
        Ok(NodeUpdate::Updated)
    }
}
