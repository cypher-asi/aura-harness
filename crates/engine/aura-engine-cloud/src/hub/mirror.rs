//! Copying placed agents' records here: after turns, on the node's hints,
//! on a sweep of awake machines and before a machine sleeps.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use aura_core_types::AgentId;

use crate::{replica, CloudResult};

use super::node_client::NodeClient;
use super::store::StoreSink;
use super::{persist, CloudHub, COPY_BEFORE_SLEEP};

impl CloudHub {
    /// Copy what `agent`'s node committed since the local head, and what
    /// the agents it spawned there committed. Returns the entries copied.
    ///
    /// # Errors
    ///
    /// When the node can't be read or a page of `agent`'s record fails
    /// its checks.
    pub async fn mirror(&self, agent: AgentId) -> CloudResult<usize> {
        let node = self.node(&agent.to_hex())?;
        self.mirror_from(&node, agent).await
    }

    /// Copy `agent`'s record from `node`, then the records of the agents
    /// it spawned there. Returns the entries copied.
    pub(super) async fn mirror_from(
        &self,
        node: &NodeClient,
        agent: AgentId,
    ) -> CloudResult<usize> {
        let hex = agent.to_hex();
        let result = self.mirror_record(node, &hex, agent).await;
        self.note_copy(agent, &result);
        let mut copied = result?;
        let mut done = HashSet::from([agent]);
        loop {
            let next: Vec<AgentId> = self
                .children_of(&hex)
                .into_iter()
                .filter(|child| done.insert(*child))
                .collect();
            if next.is_empty() {
                return Ok(copied);
            }
            for child in next {
                match self.mirror_record(node, &hex, child).await {
                    Ok(count) => copied += count,
                    Err(err) => {
                        tracing::debug!(%agent, %child, error = %err, "could not copy a spawned agent's record");
                    }
                }
            }
        }
    }

    /// Copy what `node` committed for `agent` since the local head. Agents
    /// a copied page names as spawned by `agent` are copied with placed
    /// agent `root` from then on. Returns the entries copied.
    async fn mirror_record(
        &self,
        node: &NodeClient,
        root: &str,
        agent: AgentId,
    ) -> CloudResult<usize> {
        // One mirror per agent at a time: two racing copies of the same
        // page would have the loser refused by the store.
        let gate = Arc::clone(self.mirrors.lock().entry(agent).or_default());
        let _one = gate.lock().await;
        let sink = StoreSink {
            store: Arc::clone(&self.store),
            observer: self.observer.lock().clone(),
        };
        let mut copied = 0;
        let result = loop {
            let head = match replica::ReplicaSink::head(&sink, agent) {
                Ok(head) => head,
                Err(err) => break Err(err),
            };
            let page = match node.record(agent, head + 1).await {
                Ok(page) => page,
                Err(err) => break Err(err),
            };
            if page.is_empty() {
                break Ok(copied);
            }
            match replica::apply_page(&sink, agent, &page) {
                Ok(0) => break Ok(copied),
                Ok(applied) => {
                    copied += applied;
                    self.adopt(root, persist::spawned_by(agent, &page));
                }
                Err(err) => break Err(err),
            }
        };
        if copied > 0 {
            self.copied.store(true, Ordering::Release);
        }
        result
    }

    /// Copy `agent`'s record in the background. Hints that come in while
    /// a copy runs fold into one more copy after it, so a busy node costs
    /// at most two copies in flight per agent, and the caller (the mesh
    /// socket) never waits on one.
    pub(super) fn copy_soon(self: &Arc<Self>, agent: AgentId) {
        {
            let mut copies = self.copies.lock();
            if let Some(again) = copies.get_mut(&agent) {
                *again = true;
                return;
            }
            copies.insert(agent, false);
        }
        let hub = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let Some(strong) = hub.upgrade() else { return };
                if let Err(err) = strong.mirror(agent).await {
                    tracing::debug!(%agent, error = %err, "could not copy a cloud agent's record");
                }
                let mut copies = strong.copies.lock();
                match copies.get_mut(&agent) {
                    Some(again) if *again => *again = false,
                    _ => {
                        copies.remove(&agent);
                        return;
                    }
                }
            }
        });
    }

    /// Put the idle machines to sleep, copying each awake one's record
    /// first; a machine that got work meanwhile stays up.
    pub(super) async fn sleep_idle_machines(&self) {
        for due in self.fleet.idle_machines() {
            if due.awake {
                if let Ok(agent) = AgentId::from_hex(&due.agent) {
                    self.copy_before_sleep(agent).await;
                }
            }
            if self.fleet.sleep_idle(&due).await {
                if let Some(link) = self.links.lock().get(&due.agent) {
                    link.stop.cancel();
                }
            }
        }
    }

    /// Copy every awake machine's record: hints are lost when a socket
    /// drops, and an old node image sends none.
    pub(super) fn sweep(self: &Arc<Self>) {
        let awake: Vec<String> = self
            .placed()
            .into_iter()
            .map(|(hex, _)| hex)
            .filter(|hex| {
                self.fleet
                    .view(hex)
                    .is_some_and(|view| !view.state.is_asleep())
            })
            .collect();
        for hex in awake {
            if let Ok(agent) = AgentId::from_hex(&hex) {
                self.copy_soon(agent);
            }
        }
    }

    /// Copy what `agent`'s machine holds before it sleeps, so a machine
    /// that never wakes again leaves little behind. A copy that fails is
    /// only logged: it all stays on the machine's disk and is copied when
    /// it next wakes.
    pub(super) async fn copy_before_sleep(&self, agent: AgentId) {
        let node = match self.node(&agent.to_hex()) {
            Ok(node) => node,
            Err(err) => {
                tracing::warn!(%agent, error = %err, "could not reach the machine to copy it before it slept");
                return;
            }
        };
        if tokio::time::timeout(COPY_BEFORE_SLEEP, self.persist(&node, agent))
            .await
            .is_err()
        {
            tracing::warn!(%agent, "copying before the machine slept took too long");
        }
    }
}
