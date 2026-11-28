//! Reattaching to a cloud turn that an earlier hub asked for: the node's
//! chat-run driver runs a turn whether or not a socket is attached, so a
//! turn the app quit during goes on, or ends, on its machine. A hub
//! started since finds the turn by its run id and follows it again.
//!
//! The app sees only [`CloudHub::turn_run`], [`CloudHub::reattach_turn`]
//! and [`RUN_GONE`]; a fuller follower can replace this module behind
//! them.
//!
//! # Failure modes
//!
//! A run the node no longer has (it ended and was reaped, or the machine
//! restarted) ends the stream with [`RemoteEvent::Failed`] of
//! [`RUN_GONE`]. The record, session and traces are copied either way,
//! so the copied conversation can tell whether the turn finished.

use std::sync::Arc;

use aura_core_types::AgentId;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::node_client::NodeClient;
use super::turn::{ws_request, ws_url, RemoteEvent, TurnStream, WORKING};
use super::{CloudHub, TURN_TIMEOUT};
use crate::{CloudError, CloudResult};

/// Why a reattached turn's stream ended at once: its node no longer has
/// the run.
pub const RUN_GONE: &str = "its machine no longer has the turn's run";

/// How a reattach went, before the copies.
enum Attached {
    /// The stream ran: how the turn ended, as [`TurnStream::relay_mut`] says
    Relayed(CloudResult<()>),
    /// The node has no such run
    Gone,
}

impl CloudHub {
    /// The node run carrying `agent`'s turn, while this hub streams one.
    #[must_use]
    pub fn turn_run(&self, agent: AgentId) -> Option<String> {
        self.streaming.lock().get(&agent.to_hex()).cloned()
    }

    /// Follow `agent`'s turn in node run `run`, asked for by an earlier
    /// hub, to its end, as [`Self::run_turn`] streams one it asks for:
    /// wake the machine, attach to the run, which replays what it did so
    /// far and goes on live, then copy the record, the session and the
    /// traces. A run that is gone ends with [`RUN_GONE`].
    #[must_use]
    pub fn reattach_turn(
        self: &Arc<Self>,
        agent: AgentId,
        run: String,
        cancel: CancellationToken,
    ) -> mpsc::Receiver<RemoteEvent> {
        let (tx, rx) = mpsc::channel(256);
        let hub = Arc::clone(self);
        tokio::spawn(async move {
            let hex = agent.to_hex();
            let _ = tx
                .send(RemoteEvent::Status("Reaching the cloud machine".into()))
                .await;
            let node = match hub.wake_for_turn(&hex, &tx).await {
                Ok(node) => node,
                Err(err) => {
                    let _ = tx.send(RemoteEvent::Failed(err.to_string())).await;
                    return;
                }
            };
            let guard = hub.fleet.begin_work(&hex);
            let attached = {
                let _streamed = hub.streaming_turn(&hex, run.clone());
                tokio::time::timeout(TURN_TIMEOUT, attach(&hub, &node, agent, &run, &tx, cancel))
                    .await
                    .unwrap_or(Attached::Relayed(Err(CloudError::Timeout {
                        op: "turn",
                        after: TURN_TIMEOUT,
                    })))
            };
            hub.finish_on_node(&node, agent).await;
            drop(guard);
            let why = match attached {
                Attached::Relayed(Ok(())) => None,
                Attached::Relayed(Err(err)) => Some(err.to_string()),
                Attached::Gone => {
                    tracing::info!(agent = %hex, %run, "the turn's run is gone from its machine");
                    Some(RUN_GONE.to_string())
                }
            };
            if let Some(why) = why {
                let _ = tx.send(RemoteEvent::Failed(why)).await;
            }
        });
        rx
    }
}

/// Attach to `run` on `node` and relay it to `tx` until it ends. A run
/// the node refuses, or whose socket closes before it showed anything,
/// is gone.
async fn attach(
    hub: &CloudHub,
    node: &NodeClient,
    agent: AgentId,
    run: &str,
    tx: &mpsc::Sender<RemoteEvent>,
    cancel: CancellationToken,
) -> Attached {
    let url = ws_url(node.base(), &format!("/stream/{run}"));
    let request = match ws_request(&url, node.token()) {
        Ok(request) => request,
        Err(err) => return Attached::Relayed(Err(err)),
    };
    let socket = match tokio_tungstenite::connect_async(request).await {
        Ok((socket, _)) => socket,
        Err(err) => {
            tracing::debug!(%url, error = %err, "attaching to the run was refused");
            return Attached::Gone;
        }
    };
    let (mut sink, mut stream) = socket.split();
    let _ = tx.send(RemoteEvent::Status(WORKING.into())).await;
    let mut turn = TurnStream {
        hub,
        node,
        agent,
        url,
        sink: &mut sink,
        stream: &mut stream,
        tx,
        relayed: false,
    };
    let ended = turn.relay_mut(cancel).await;
    if ended.is_err() && !turn.relayed {
        return Attached::Gone;
    }
    Attached::Relayed(ended)
}
