//! Picking up a turn an earlier hub asked for, on a machine that is
//! already running it ([`CloudHub::reconnect_for_turn`]).
//!
//! A turn under way means its machine is awake: the hub only needs a token
//! for its node (a claim, since tokens aren't kept across restarts) and a
//! socket to it. So the pickup never wakes the machine and never checks it
//! for an update (#76): it reconnects.
//!
//! ## Invariants
//!
//! - No wake call for a machine that is up. Only one that sleeps is woken,
//!   as its turn ended there and its reply must still be copied (today's
//!   path, [`CloudHub::wake_for_turn`]).
//! - At most [`RECONNECTS_AT_ONCE`] reconnects run at once, so a restart
//!   with many turns doesn't claim every node at the same moment; the
//!   streams that follow aren't limited.
//! - A node that can't be reached is tried again after each of the delays
//!   in [`RECONNECT_DELAYS`], then the turn is reported failed with its
//!   mark kept, as before.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use super::node_client::NodeClient;
use super::turn::RemoteEvent;
use super::CloudHub;
use crate::boot::BootReporter;
use crate::error::{CloudError, CloudResult};

/// What a picked-up turn's chat and its agent's row say while the hub
/// reconnects to its machine.
pub const RECONNECTING: &str = "Reconnecting…";
/// What they say once it is reconnected and follows the turn.
pub const RECONNECTED: &str = "Following its turn on the cloud machine";
/// Reconnects at once.
pub const RECONNECTS_AT_ONCE: usize = 8;
/// How long to wait before each new try to reach a node.
pub const RECONNECT_DELAYS: [Duration; 4] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(30),
];

impl CloudHub {
    /// Reach `hex`'s node to follow the turn running on it: reconnect to
    /// a machine that is up, retrying with backoff, and wake only one that
    /// sleeps. Shows the claim's progress on `tx`.
    ///
    /// # Errors
    ///
    /// When the node can't be reached after every try.
    pub(super) async fn reconnect_for_turn(
        self: &Arc<Self>,
        hex: &str,
        tx: &mpsc::Sender<RemoteEvent>,
    ) -> CloudResult<NodeClient> {
        let _ = tx.send(RemoteEvent::Status(RECONNECTING.into())).await;
        let reached = {
            // Closed only as the hub stops, which ends this turn anyway.
            let Ok(_slot) = Arc::clone(&self.reconnects).acquire_owned().await else {
                return Err(stopping(hex));
            };
            self.reconnect_retrying(hex, tx).await
        };
        match reached {
            Ok(Some(node)) => {
                self.ensure_link(hex);
                let _ = tx.send(RemoteEvent::Status(RECONNECTED.into())).await;
                Ok(node)
            }
            Ok(None) => {
                tracing::info!(agent = %hex, "a marked cloud turn's machine sleeps; waking it to copy the turn");
                self.wake_for_turn(hex, tx).await
            }
            Err(err) => Err(err),
        }
    }

    /// [`Self::reconnect_once`], tried again after each of the delays;
    /// `Ok(None)` when the machine sleeps.
    async fn reconnect_retrying(
        &self,
        hex: &str,
        tx: &mpsc::Sender<RemoteEvent>,
    ) -> CloudResult<Option<NodeClient>> {
        let delays = self.reconnect_delays.lock().clone();
        let mut last = None;
        for wait in std::iter::once(Duration::ZERO).chain(delays) {
            if !wait.is_zero() {
                tokio::select! {
                    () = self.stop.cancelled() => break,
                    () = tokio::time::sleep(wait) => {}
                }
            }
            match self.reconnect_once(hex, tx).await {
                Ok(reached) => return Ok(reached),
                Err(err) => {
                    tracing::info!(agent = %hex, error = %err, retry_in = ?wait, "could not reconnect to a cloud machine yet");
                    last = Some(err);
                }
            }
        }
        Err(last.unwrap_or_else(|| stopping(hex)))
    }

    /// One try: claim `hex`'s node if needed, without waking its machine.
    async fn reconnect_once(
        &self,
        hex: &str,
        tx: &mpsc::Sender<RemoteEvent>,
    ) -> CloudResult<Option<NodeClient>> {
        let (boot, mut events) = BootReporter::channel();
        let shown = tx.clone();
        let forward = tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                if shown.send(RemoteEvent::Boot(event)).await.is_err() {
                    break;
                }
            }
        });
        let reached = self.fleet.reconnect_reported(hex, &boot).await;
        drop(boot);
        let _ = forward.await;
        if reached?.is_none() {
            return Ok(None);
        }
        self.fleet.check_node_protocol(hex)?;
        self.node(hex).map(Some)
    }

    /// Use `delays` between tries to reach a node from now on: for tests,
    /// which can't wait the real ones.
    #[doc(hidden)]
    pub fn set_reconnect_delays(&self, delays: Vec<Duration>) {
        *self.reconnect_delays.lock() = delays;
    }
}

/// Why a reconnect of `hex`'s node ended without a try: the hub stops.
fn stopping(hex: &str) -> CloudError {
    CloudError::Node {
        node: hex.to_string(),
        op: "reconnect",
        message: "the hub is stopping".into(),
    }
}
