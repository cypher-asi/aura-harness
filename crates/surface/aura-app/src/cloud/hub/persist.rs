//! What the hub keeps of a cloud agent besides its own record, so little
//! is lost with its machine: the records of agents it spawned there, its
//! session checkpoint, its trace runs and, at each sleep, a patch of its
//! changes; and how current each copy is ([`SyncStatus`]).

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use aura_agent::checkpoint::CHECKPOINT_VERSION;
use aura_agent_trace::db::{self, RunCopy};
use aura_agent_trace::RunMeta;
use aura_core_protocol::cloud::WorkspacePatch;
use aura_core_types::{AgentId, RecordEntry, TransactionType};
use aura_engine_cloud::{CloudError, CloudResult};
use aura_store_db::TraceStore;
use serde::{Deserialize, Serialize};

use super::{io, now_ms, CloudHub};
use crate::cloud::node_client::NodeClient;

/// The patch saved at a machine's last sleep, in the agent's folder
/// under the hub's home.
const SAVED_PATCH: &str = "at-sleep.json";

/// How current this computer's copy of a cloud agent is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncStatus {
    /// Head of the agent's record here.
    pub copied_seq: u64,
    /// When a copy last reached the node (ms since the epoch).
    pub copied_ms: Option<u64>,
    /// Head the node last reported for the agent.
    pub node_seq: Option<u64>,
    /// Why the last copy failed, until one succeeds.
    pub error: Option<String>,
    /// Agents it spawned on its machine, copied with it.
    pub children: usize,
    /// When its session checkpoint was last saved here.
    pub session_ms: Option<u64>,
    /// When its trace runs were last copied.
    pub traces_ms: Option<u64>,
    /// When a patch of its machine's changes was last saved.
    pub patch_ms: Option<u64>,
}

impl SyncStatus {
    /// Whether the machine holds entries this computer doesn't, or the
    /// last copy failed.
    #[must_use]
    pub fn behind(&self) -> bool {
        self.error.is_some() || self.node_seq.is_some_and(|node| node > self.copied_seq)
    }
}

/// The changes a machine held when it last slept.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SavedPatch {
    pub(super) saved_ms: u64,
    pub(super) patch: WorkspacePatch,
}

/// What a copied run held when it was stored, so an unchanged run is
/// not stored again.
pub(super) type RunMark = (usize, usize, usize, bool, Option<u64>);

fn mark(run: &RunCopy) -> RunMark {
    (
        run.steps.len(),
        run.records.len(),
        run.rewards.len(),
        run.summary.is_some(),
        run.meta.ended_at_ms,
    )
}

/// The agents that `entries` of `parent`'s record name as spawned by it.
pub(super) fn spawned_by(parent: AgentId, entries: &[RecordEntry]) -> Vec<AgentId> {
    #[derive(Deserialize)]
    struct Spawn {
        kind: String,
        parent_agent_id: AgentId,
        child_agent_id: AgentId,
    }
    entries
        .iter()
        .filter(|entry| entry.tx.tx_type == TransactionType::System)
        .filter_map(|entry| serde_json::from_slice::<Spawn>(&entry.tx.payload).ok())
        .filter(|spawn| {
            spawn.kind == "spawn_agent"
                && spawn.parent_agent_id == parent
                && spawn.child_agent_id != parent
        })
        .map(|spawn| spawn.child_agent_id)
        .collect()
}

/// When the file at `path` was last written (ms since the epoch).
fn written_ms(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    u64::try_from(since.as_millis()).ok()
}

/// Store each of `runs` (agent `hex`'s) in place of its earlier copy,
/// except over a run another agent owns. Returns what was stored.
fn store_runs(store: &Arc<TraceStore>, hex: &str, runs: Vec<RunCopy>) -> Vec<(String, RunMark)> {
    runs.into_iter()
        .filter(|run| {
            store
                .meta(&run.meta.run_id)
                .ok()
                .flatten()
                .and_then(|bytes| serde_json::from_slice::<RunMeta>(&bytes).ok())
                .and_then(|meta| db::owner_of(&meta))
                .is_none_or(|owner| owner == hex)
        })
        .filter_map(|run| match db::replace(store, &run) {
            Ok(()) => Some((run.meta.run_id.clone(), mark(&run))),
            Err(err) => {
                tracing::debug!(error = %err, "could not store a copied trace run");
                None
            }
        })
        .collect()
}

impl CloudHub {
    /// Hand the hub the main trace store, where cloud agents' runs are
    /// copied.
    pub fn attach_traces(&self, store: Arc<TraceStore>) {
        *self.trace_store.lock() = Some(store);
    }

    /// How current this computer's copy of `agent` is; `None` when it is
    /// not placed.
    #[must_use]
    pub fn sync_status(&self, agent: AgentId) -> Option<SyncStatus> {
        let hex = agent.to_hex();
        let children = self.placements.lock().agents.get(&hex)?.children.len();
        let mut status = self.sync.lock().get(&agent).cloned().unwrap_or_default();
        status.children = children;
        if let Ok(head) = self.store.get_head_seq(agent) {
            status.copied_seq = head;
        }
        if status.patch_ms.is_none() {
            status.patch_ms = written_ms(&self.home.join(&hex).join(SAVED_PATCH));
        }
        Some(status)
    }

    fn sync_mut(&self, agent: AgentId, update: impl FnOnce(&mut SyncStatus)) {
        update(self.sync.lock().entry(agent).or_default());
    }

    /// The lock held by each copy for `agent`: a run is stored by
    /// deleting its earlier copy, so two copies at once could store it
    /// twice.
    fn copy_gate(&self, agent: AgentId) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(self.mirrors.lock().entry(agent).or_default())
    }

    /// Note the node's word that `agent`'s record reached `head`.
    pub(super) fn note_head(&self, agent: AgentId, head: u64) {
        self.sync_mut(agent, |status| status.node_seq = Some(head));
    }

    /// Note how a copy of `agent`'s record went.
    pub(super) fn note_copy(&self, agent: AgentId, result: &CloudResult<usize>) {
        self.sync_mut(agent, |status| match result {
            Ok(_) => {
                status.copied_ms = Some(now_ms());
                status.error = None;
            }
            Err(err) => status.error = Some(err.to_string()),
        });
    }

    /// The spawned agents whose records are copied with placed agent
    /// `hex`'s.
    pub(super) fn children_of(&self, hex: &str) -> Vec<AgentId> {
        self.placements
            .lock()
            .agents
            .get(hex)
            .map(|placed| {
                placed
                    .children
                    .iter()
                    .filter_map(|child| AgentId::from_hex(child).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Copy the agents in `named` along with placed agent `hex` from now
    /// on; they were named as spawned in a record that was just verified.
    /// One that already has a record here, or is placed itself, is not
    /// taken: a node can't pull another agent's record in this way.
    pub(super) fn adopt(&self, hex: &str, named: Vec<AgentId>) {
        if named.is_empty() {
            return;
        }
        let mut added = false;
        {
            let mut placements = self.placements.lock();
            let placed: HashSet<String> = placements.agents.keys().cloned().collect();
            let Some(parent) = placements.agents.get_mut(hex) else {
                return;
            };
            for child in named {
                let id = child.to_hex();
                if parent.children.contains(&id) {
                    continue;
                }
                let fresh = self.store.get_head_seq(child).is_ok_and(|head| head == 0);
                if placed.contains(&id) || !fresh {
                    tracing::warn!(agent = %hex, child = %id, "the machine named an agent that already exists here as spawned; not copying it");
                    continue;
                }
                parent.children.push(id);
                added = true;
            }
        }
        if added {
            self.save_placements();
        }
    }

    /// Save `agent`'s session checkpoint from its node here, so Bring
    /// back, or a lost machine, resumes the whole session, tool calls
    /// included. One past the record copied so far is not saved: the
    /// restore here would not use it. Returns whether one was saved.
    pub(super) async fn copy_session(
        &self,
        node: &NodeClient,
        agent: AgentId,
    ) -> CloudResult<bool> {
        let Some(sessions) = self.sessions.lock().clone() else {
            return Ok(false);
        };
        let gate = self.copy_gate(agent);
        let _one = gate.lock().await;
        let Some(checkpoint) = node.session(agent).await? else {
            return Ok(false);
        };
        let head = self
            .store
            .get_head_seq(agent)
            .map_err(|e| CloudError::io("read record", std::io::Error::other(e.to_string())))?;
        if checkpoint.version != CHECKPOINT_VERSION || checkpoint.record_seq > head {
            return Ok(false);
        }
        sessions
            .save(agent, &checkpoint)
            .await
            .map_err(io("save the session checkpoint"))?;
        self.sync_mut(agent, |status| status.session_ms = Some(now_ms()));
        Ok(true)
    }

    /// Copy `agent`'s trace runs from its node into the main trace store,
    /// skipping runs unchanged since the last copy. Only the agent's own
    /// runs are taken. Returns how many were stored.
    pub(super) async fn copy_traces(
        &self,
        node: &NodeClient,
        agent: AgentId,
    ) -> CloudResult<usize> {
        let Some(store) = self.trace_store.lock().clone() else {
            return Ok(0);
        };
        let gate = self.copy_gate(agent);
        let _one = gate.lock().await;
        let hex = agent.to_hex();
        let page = node.traces(agent).await?;
        let changed: Vec<RunCopy> = {
            let marks = self.run_marks.lock();
            page.runs
                .into_iter()
                .filter_map(|run| serde_json::from_value::<RunCopy>(run).ok())
                .filter(|run| db::owner_of(&run.meta).as_deref() == Some(hex.as_str()))
                .filter(|run| marks.get(&run.meta.run_id) != Some(&mark(run)))
                .collect()
        };
        let stored = if changed.is_empty() {
            Vec::new()
        } else {
            tokio::task::spawn_blocking(move || store_runs(&store, &hex, changed))
                .await
                .map_err(|e| CloudError::io("copy trace runs", std::io::Error::other(e)))?
        };
        let count = stored.len();
        if count > 0 {
            self.run_marks.lock().extend(stored);
            self.traces_copied.lock().insert(agent);
        }
        self.sync_mut(agent, |status| status.traces_ms = Some(now_ms()));
        Ok(count)
    }

    /// Save the changes on `agent`'s machine here, for Bring back to use
    /// if the machine is lost.
    pub(super) async fn save_patch(&self, node: &NodeClient, agent: AgentId) -> CloudResult<()> {
        let saved = SavedPatch {
            saved_ms: now_ms(),
            patch: node.patch().await?,
        };
        let dir = self.home.join(agent.to_hex());
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(io("save the machine's changes"))?;
        let bytes = serde_json::to_vec(&saved)
            .map_err(|e| CloudError::io("save the machine's changes", std::io::Error::other(e)))?;
        let path = dir.join(SAVED_PATCH);
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, bytes)
            .await
            .map_err(io("save the machine's changes"))?;
        tokio::fs::rename(&tmp, &path)
            .await
            .map_err(io("save the machine's changes"))?;
        self.sync_mut(agent, |status| status.patch_ms = Some(saved.saved_ms));
        Ok(())
    }

    /// The changes saved at placed agent `hex`'s machine's last sleep.
    pub(super) fn saved_patch(&self, hex: &str) -> Option<SavedPatch> {
        let bytes = std::fs::read(self.home.join(hex).join(SAVED_PATCH)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Drop the changes saved for `hex`, once the agent left the cloud.
    pub(super) fn forget_saved_patch(&self, hex: &str) {
        let _ = std::fs::remove_file(self.home.join(hex).join(SAVED_PATCH));
    }

    /// Drop the changes saved at `agent`'s machine's last sleep once a
    /// pull brought the machine's work into the project; the next sleep
    /// saves what came after.
    pub(super) fn saved_patch_pulled(&self, agent: AgentId) {
        self.forget_saved_patch(&agent.to_hex());
        self.sync_mut(agent, |status| status.patch_ms = None);
    }

    /// Copy everything `agent`'s machine holds that this computer needs
    /// if the machine is lost: the records, the session checkpoint, the
    /// trace runs and a patch of its changes. A part that fails is only
    /// logged and the rest still runs: the machine's disk keeps it all,
    /// and HOST shows the agent as behind.
    pub(super) async fn persist(&self, node: &NodeClient, agent: AgentId) {
        if let Err(err) = self.mirror_from(node, agent).await {
            tracing::warn!(%agent, error = %err, "could not copy the record");
        }
        if let Err(err) = self.copy_session(node, agent).await {
            tracing::warn!(%agent, error = %err, "could not copy the session checkpoint");
        }
        if let Err(err) = self.copy_traces(node, agent).await {
            tracing::warn!(%agent, error = %err, "could not copy the trace runs");
        }
        if let Err(err) = self.save_patch(node, agent).await {
            tracing::warn!(%agent, error = %err, "could not save the machine's changes");
        }
    }
}
