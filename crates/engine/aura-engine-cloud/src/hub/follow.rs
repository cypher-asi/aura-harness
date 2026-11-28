//! Following a turn's chat run on its node to its end: on the socket the
//! turn was asked on, on sockets attached again when one drops, and from
//! a hub started after a crash or a quit, which finds the turn by the
//! mark on its placement ([`CloudHub::follow_turn`]). The node runs the
//! turn whether or not a socket is attached.
//!
//! A turn's mark goes only once its run is known to have ended: its end
//! was seen, the node says the run is gone, or the hub stopped it. When
//! the hub stops watching for another reason (the machine can't be woken,
//! the node stopped answering, the turn ran past [`TURN_TIMEOUT`] and
//! couldn't be stopped), the turn is reported failed and its mark stays
//! ([`CloudHub::unsettled_run`]), so the node is never restarted under a
//! turn that may still run. A later turn's mark replaces it.

use std::sync::Arc;
use std::time::Duration;

use aura_core_protocol::{InboundMessage, OutboundMessage};
use aura_core_types::time::now_ms;
use aura_core_types::{AgentId, TransactionType};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::{CloudError, CloudResult};

use super::marks::TurnMark;
use super::node_client::NodeClient;
use super::turn::{connect_run, map_outbound, NodeSocket, RemoteEvent};
use super::{CloudHub, MIRROR_EVERY, TURN_TIMEOUT};

/// First wait before attaching to a run again; it doubles each time, up
/// to [`LONGEST_RETRY`].
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LONGEST_RETRY: Duration = Duration::from_secs(30);
/// How long a node may go unanswering before the turn counts as lost.
const GIVE_UP_AFTER: Duration = Duration::from_secs(15 * 60);
/// Entries read at a time looking for a turn's reply.
const SCAN_BATCH: usize = 500;

/// How a turn the hub watched came to an end.
pub(super) enum StreamEnd {
    /// The turn is over as the result says, or the hub stopped watching
    /// it on purpose (the user stopped it, it ran too long, its node
    /// stopped answering).
    Seen(CloudResult<()>),
    /// The hub let go of it as it ran on: the hub stopped, or no one
    /// listens any more. Its mark stays for the next hub.
    Left,
}

/// What a turn's chat is sent of its stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shown {
    /// Every event, as it streams.
    Live,
    /// How it ends and its title, and [`RemoteEvent::Copied`] as its
    /// entries come in: an attached socket replays what may have been
    /// shown already.
    Quiet,
}

/// How one socket of a run ended.
enum Relayed {
    Ended(StreamEnd),
    /// It closed or broke first; the run may well go on.
    Dropped(CloudError),
}

/// A chat run on a node carrying `agent`'s marked turn.
pub(super) struct RunWatch<'a> {
    pub(super) hub: &'a CloudHub,
    pub(super) node: &'a NodeClient,
    pub(super) agent: AgentId,
    pub(super) run_id: &'a str,
    /// The record's head before the turn: its reply comes after it.
    pub(super) head: u64,
}

impl CloudHub {
    /// Follow `agent`'s turn `mark`, asked for by an earlier hub, to its
    /// end, as [`Self::run_turn`] streams one it asks for: reconnect to
    /// the machine ([`Self::reconnect_for_turn`]), attach to the turn's run and send how the turn ends,
    /// with [`RemoteEvent::Copied`] each time entries of it came in. A
    /// run that is gone ended while no hub watched; the record says
    /// whether it replied. The record, session and runs are copied when
    /// it ends, and the mark goes.
    #[must_use]
    pub fn follow_turn(
        self: &Arc<Self>,
        agent: AgentId,
        mark: TurnMark,
        cancel: CancellationToken,
    ) -> mpsc::Receiver<RemoteEvent> {
        let (tx, rx) = mpsc::channel(256);
        let hub = Arc::clone(self);
        tokio::spawn(async move {
            let hex = agent.to_hex();
            // Its machine runs the turn: reconnect, never wake or update.
            let node = match hub.reconnect_for_turn(&hex, &tx).await {
                Ok(node) => node,
                Err(err) => {
                    // The run may well go on there: its mark stays for
                    // a later look, and the node isn't restarted under it.
                    tracing::info!(agent = %hex, run = %mark.run_id, error = %err, "could not reach a marked cloud turn; its mark stays");
                    let _ = tx.send(RemoteEvent::Failed(err.to_string())).await;
                    return;
                }
            };
            let _streamed = hub.streaming_turn(&hex, mark.run_id.clone());
            let guard = hub.fleet.begin_work(&hex);
            let watch = RunWatch {
                hub: &hub,
                node: &node,
                agent,
                run_id: &mark.run_id,
                head: mark.head,
            };
            let end = if watch.copy(Shown::Quiet, &tx).await {
                let deadline = deadline_from(mark.started_ms);
                watch
                    .follow(None, Shown::Quiet, &tx, &cancel, deadline)
                    .await
            } else {
                StreamEnd::Left
            };
            hub.finish_turn(&node, agent, Some(&mark.run_id), end, guard, &tx)
                .await;
        });
        rx
    }

    /// Whether `agent`'s record here has an agent message after `head`.
    fn replied_since(&self, agent: AgentId, head: u64) -> bool {
        let mut from = head + 1;
        loop {
            let Ok(entries) = self.store.scan_record(agent, from, SCAN_BATCH) else {
                return false;
            };
            if entries
                .iter()
                .any(|entry| entry.tx.tx_type == TransactionType::AgentMsg)
            {
                return true;
            }
            match entries.last() {
                Some(last) if entries.len() == SCAN_BATCH => from = last.seq + 1,
                _ => return false,
            }
        }
    }
}

/// When a turn asked for at `started_ms` has run too long.
fn deadline_from(started_ms: u64) -> Instant {
    let limit = u64::try_from(TURN_TIMEOUT.as_millis()).unwrap_or(u64::MAX);
    let left = started_ms.saturating_add(limit).saturating_sub(now_ms());
    Instant::now() + Duration::from_millis(left)
}

fn timed_out_error() -> CloudError {
    CloudError::Timeout {
        op: "turn",
        after: TURN_TIMEOUT,
    }
}

impl RunWatch<'_> {
    /// Follow the run until its turn ends or `deadline` passes: on
    /// `socket` while it lasts, then on sockets attached again, which
    /// are [`Shown::Quiet`].
    pub(super) async fn follow(
        &self,
        socket: Option<NodeSocket>,
        mut shown: Shown,
        tx: &mpsc::Sender<RemoteEvent>,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> StreamEnd {
        let mut socket = socket;
        loop {
            let open = match socket.take() {
                Some(open) => open,
                None => match self.attach(tx, cancel, deadline).await {
                    Ok(open) => open,
                    Err(end) => return end,
                },
            };
            let relayed =
                tokio::time::timeout_at(deadline, self.relay(open, shown, tx, cancel)).await;
            match relayed {
                Err(_) => return self.timed_out().await,
                Ok(Relayed::Ended(end)) => return end,
                Ok(Relayed::Dropped(err)) => {
                    tracing::info!(agent = %self.agent, run = self.run_id, error = %err, "a cloud turn's stream dropped; attaching to it again");
                    shown = Shown::Quiet;
                    let status = RemoteEvent::Status("Reconnecting to the cloud machine".into());
                    if tx.send(status).await.is_err() {
                        return StreamEnd::Left;
                    }
                }
            }
        }
    }

    /// A socket attached to the run again, once its node answers. When
    /// the run is gone its turn ended unseen ([`Self::ended_unseen`]).
    async fn attach(
        &self,
        tx: &mpsc::Sender<RemoteEvent>,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<NodeSocket, StreamEnd> {
        let mut wait = FIRST_RETRY;
        let give_up = Instant::now() + GIVE_UP_AFTER;
        loop {
            if cancel.is_cancelled() {
                // No socket to say it on.
                return Err(self.stop_or_keep(self.stopped()).await);
            }
            match self.node.run_alive(self.run_id).await {
                Ok(false) => return Err(self.ended_unseen(tx).await),
                Ok(true) => match connect_run(self.node, self.run_id).await {
                    Ok(socket) => {
                        let status = RemoteEvent::Status("Working in the cloud".into());
                        let _ = tx.send(status).await;
                        return Ok(socket);
                    }
                    Err(err) => {
                        tracing::debug!(run = self.run_id, error = %err, "could not attach to a cloud run");
                    }
                },
                Err(err) => {
                    tracing::debug!(run = self.run_id, error = %err, "could not ask after a cloud run");
                }
            }
            let next = Instant::now() + wait;
            if next >= deadline {
                return Err(self.timed_out().await);
            }
            if next >= give_up {
                // The node stopped answering: the run may go on there.
                return Err(self.unsettled(self.closed()));
            }
            tokio::select! {
                () = cancel.cancelled() => {}
                () = self.hub.stop.cancelled() => return Err(StreamEnd::Left),
                () = tokio::time::sleep_until(next) => {}
            }
            wait = (wait * 2).min(LONGEST_RETRY);
        }
    }

    /// The turn ran past [`TURN_TIMEOUT`]: stop its run, its mark kept
    /// unless the run is then known to be gone.
    async fn timed_out(&self) -> StreamEnd {
        self.stop_or_keep(timed_out_error()).await
    }

    /// Stop the run and end the turn as `why`. Its mark goes only once
    /// the node says the run is gone.
    async fn stop_or_keep(&self, why: CloudError) -> StreamEnd {
        if let Err(err) = self.node.stop_run(self.run_id).await {
            tracing::debug!(run = self.run_id, error = %err, "could not stop a cloud run");
        }
        match self.node.run_alive(self.run_id).await {
            Ok(false) => StreamEnd::Seen(Err(why)),
            _ => self.unsettled(why),
        }
    }

    /// End the turn as `why` without knowing its run ended: its mark
    /// stays ([`CloudHub::unsettled_run`]).
    fn unsettled(&self, why: CloudError) -> StreamEnd {
        tracing::warn!(agent = %self.agent, run = self.run_id, error = %why, "stopped watching a cloud turn that may still run; its mark stays");
        self.hub.unsettled_run(self.run_id);
        StreamEnd::Seen(Err(why))
    }

    /// How the turn ended while no socket watched, its run gone: copy
    /// the record, then it replied if an agent message follows the
    /// turn's start.
    async fn ended_unseen(&self, tx: &mpsc::Sender<RemoteEvent>) -> StreamEnd {
        if !self.copy(Shown::Quiet, tx).await {
            return StreamEnd::Left;
        }
        if self.hub.replied_since(self.agent, self.head) {
            let _ = tx.send(RemoteEvent::Done(Box::default())).await;
            return StreamEnd::Seen(Ok(()));
        }
        StreamEnd::Seen(Err(CloudError::Node {
            node: self.node.node().to_string(),
            op: "turn",
            message: "it ended while no app watched it, without a reply".into(),
        }))
    }

    /// Show what `socket` carries of the turn as `shown` says, copying
    /// the record as it runs, until the turn ends or the socket drops.
    async fn relay(
        &self,
        socket: NodeSocket,
        shown: Shown,
        tx: &mpsc::Sender<RemoteEvent>,
        cancel: &CancellationToken,
    ) -> Relayed {
        let (mut sink, mut stream) = socket.split();
        let mut mirror = tokio::time::interval(MIRROR_EVERY);
        mirror.tick().await;
        loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    let cancel = serde_json::to_string(&InboundMessage::Cancel).unwrap_or_default();
                    let _ = sink.send(Message::Text(cancel.into())).await;
                    return Relayed::Ended(StreamEnd::Seen(Err(self.stopped())));
                }
                () = self.hub.stop.cancelled() => return Relayed::Ended(StreamEnd::Left),
                _ = mirror.tick() => {
                    if !self.copy(shown, tx).await {
                        return Relayed::Ended(StreamEnd::Left);
                    }
                }
                frame = stream.next() => match frame {
                    None => return Relayed::Dropped(self.closed()),
                    Some(Err(err)) => return Relayed::Dropped(CloudError::Transport { endpoint: self.node.base().to_string(), message: err.to_string() }),
                    Some(Ok(frame)) => {
                        if let Some(end) = self.take_frame(frame, shown, tx).await {
                            return Relayed::Ended(end);
                        }
                    }
                },
            }
        }
    }

    /// Pass on one frame as `shown` says; `Some` once the turn is over or
    /// no one listens.
    async fn take_frame(
        &self,
        frame: Message,
        shown: Shown,
        tx: &mpsc::Sender<RemoteEvent>,
    ) -> Option<StreamEnd> {
        let Message::Text(text) = frame else {
            return None;
        };
        let message = serde_json::from_str::<OutboundMessage>(&text).ok()?;
        match map_outbound(message)? {
            RemoteEvent::Failed(err) => Some(StreamEnd::Seen(Err(CloudError::Node {
                node: self.node.node().to_string(),
                op: "turn",
                message: err,
            }))),
            event @ RemoteEvent::Done(_) => {
                let _ = tx.send(event).await;
                Some(StreamEnd::Seen(Ok(())))
            }
            event @ RemoteEvent::Title(_) => {
                tx.send(event).await.is_err().then_some(StreamEnd::Left)
            }
            event if shown == Shown::Live => {
                tx.send(event).await.is_err().then_some(StreamEnd::Left)
            }
            _ => None,
        }
    }

    /// Copy what the node wrote since the last copy, saying so to a
    /// [`Shown::Quiet`] chat. Returns whether anyone still listens.
    async fn copy(&self, shown: Shown, tx: &mpsc::Sender<RemoteEvent>) -> bool {
        match self.hub.mirror_from(self.node, self.agent).await {
            Ok(copied) if copied > 0 && shown == Shown::Quiet => {
                tx.send(RemoteEvent::Copied).await.is_ok()
            }
            Ok(_) => !tx.is_closed(),
            Err(err) => {
                tracing::debug!(agent = %self.agent, error = %err, "could not copy a cloud turn's record");
                !tx.is_closed()
            }
        }
    }

    fn stopped(&self) -> CloudError {
        CloudError::BadState {
            machine: self.node.node().to_string(),
            state: "cancelled".into(),
            op: "finish the turn",
        }
    }

    fn closed(&self) -> CloudError {
        CloudError::Node {
            node: self.node.node().to_string(),
            op: "stream",
            message: "closed before the turn ended".into(),
        }
    }
}
