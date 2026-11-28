//! Catching up when the hub starts on what placed agents' machines made
//! while no hub watched them: the machines that run are claimed and
//! everything each holds is copied here. A machine with a turn to follow
//! is left to [`CloudHub::follow_turn`].
//!
//! A sleeping machine is never woken for this: with nothing running on
//! it there is no new work for it. What it made since a hub last copied
//! it is copied when it next wakes for work, as its link opens
//! (`copy_soon`).

use std::sync::Arc;

use aura_core_types::AgentId;
use futures_util::StreamExt;

use crate::BootReporter;

use super::marks::slept_unchanged;
use super::CloudHub;

/// Machines caught up on at once.
const AT_ONCE: usize = 3;

impl CloudHub {
    /// Copy what placed agents' machines made while no hub watched them,
    /// a few machines at a time ([`Self::catch_up_due`]).
    pub(super) async fn catch_up(self: &Arc<Self>) {
        let due = self.catch_up_due();
        if due.is_empty() {
            return;
        }
        tracing::info!(
            machines = due.len(),
            "copying what cloud machines made while no hub watched"
        );
        futures_util::stream::iter(due)
            .for_each_concurrent(AT_ONCE, |hex| async move { self.catch_up_on(&hex).await })
            .await;
    }

    /// The placed agents whose machines may hold what this computer does
    /// not: those that run (they may be working), and sleeping ones that
    /// no hub put to sleep with everything copied, or that something
    /// reached since. Agents with a turn mark are left out.
    pub(super) fn catch_up_due(&self) -> Vec<String> {
        self.placed()
            .into_iter()
            .filter(|(hex, placed)| {
                placed.turn.is_none()
                    && self.fleet.view(hex).is_some_and(|view| {
                        !view.state.is_asleep()
                            || !slept_unchanged(placed.slept_ms, view.machine.last_connected_ms)
                    })
            })
            .map(|(hex, _)| hex)
            .collect()
    }

    /// Claim `hex`'s machine if it is up, keep a socket to it open and
    /// copy all it holds. One that sleeps is left asleep.
    async fn catch_up_on(self: &Arc<Self>, hex: &str) {
        if self.stop.is_cancelled() {
            return;
        }
        let Ok(agent) = AgentId::from_hex(hex) else {
            return;
        };
        let reached = self
            .fleet
            .reconnect_reported(hex, &BootReporter::none())
            .await
            .and_then(|up| up.map(|_| self.node(hex)).transpose());
        let node = match reached {
            Ok(Some(node)) => node,
            Ok(None) => {
                tracing::info!(agent = %hex, "a sleeping cloud machine is left asleep; what it made is copied when it next wakes");
                return;
            }
            Err(err) => {
                tracing::warn!(agent = %hex, error = %err, "could not reach a cloud machine to copy what it made");
                return;
            }
        };
        self.ensure_link(hex);
        let _awake = self.fleet.begin_work(hex);
        if !self.persist(&node, agent).await {
            tracing::warn!(agent = %hex, "a cloud machine's copy is still behind");
        }
    }
}
