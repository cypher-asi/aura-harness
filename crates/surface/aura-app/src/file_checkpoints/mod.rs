//! Per-turn file checkpoints for the terminal.
//!
//! Before an agent's file tools first change a file in a turn, the file's
//! content (or its absence) is saved, so `/rewind` can put files back and
//! `/diff` can show what a turn or the session changed. Contents are stored
//! once per hash under `<data_dir>/checkpoints/<agent>/blobs/`, and each
//! turn has a manifest at `<data_dir>/checkpoints/<agent>/turns/<ms>.json`.
//!
//! Only the file tools are seen (through [`FileChangeHook`]); files changed
//! by shell commands are neither checkpointed nor restored.
//!
//! Separately, one subtask's attempt can be watched
//! ([`FileCheckpoints::begin_attempt`]): what each file held before the
//! attempt (or a subagent of it) first changed it is kept in memory, so
//! the attempt's changes can be put back ([`AttemptFiles::restore`])
//! before it is retried.

use crate::event_loop::agent_events::AgentUi;
use async_trait::async_trait;
use aura_core_types::{AgentId, Hash};
use aura_exec_tools::FileChangeHook;
use aura_store_snapshot::{DirSnapshotStore, SnapshotStore};
use aura_ui::events::FileDiffData;
use aura_ui::UiCommand;
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::sync::mpsc;
use tracing::warn;

mod attempt;
mod helpers;
mod hook;

pub use helpers::unified_diff;
use helpers::{display_path, earliest, hash, preview, read_optional, write_file};
use hook::CheckpointHook;

/// Lock `mutex`, going on after a panic elsewhere: the maps it guards stay
/// consistent between statements.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Format version of turn manifests.
const VERSION: u32 = 1;

/// Characters of the prompt kept in a manifest for the `/rewind` list.
const PROMPT_CHARS: usize = 80;

/// Files larger than this (before and after together) are reported as
/// changed without a line diff, which would be slow and unreadable.
const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;

/// What a file held before a turn first changed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBefore {
    pub path: PathBuf,
    /// Hash of the saved content; `None` when the file did not exist
    pub blob: Option<String>,
}

/// One turn of an agent: what it was asked and the files it changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnManifest {
    pub version: u32,
    /// Session the turn belongs to (its `started_ms`)
    pub session_started_ms: u64,
    /// When the turn started; names its manifest file
    pub started_ms: u64,
    /// Where the turn's prompt sits in the session's conversation
    pub message_index: usize,
    /// The prompt's first line, cut short
    pub prompt: String,
    /// Hash of the whole prompt, to find it again in the conversation
    pub prompt_hash: String,
    /// Files in the order the turn first changed them
    pub files: Vec<FileBefore>,
    /// The conversation was rewound to before this turn
    #[serde(default)]
    pub rewound: bool,
    /// Hex hash of the turn's prompt transaction in the agent's record,
    /// once it is recorded
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tx: Option<String>,
}

impl TurnManifest {
    /// Whether `text` is this turn's prompt.
    #[must_use]
    pub fn is_prompt(&self, text: &str) -> bool {
        hash(text.as_bytes()) == self.prompt_hash
    }
}

/// A turn still in the conversation, numbered from 1 within its session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub number: usize,
    pub manifest: TurnManifest,
}

/// What a file held at a checkpoint: its content, `None` when it did not
/// exist, or why the checkpoint can't be read.
pub type Held = Result<Option<Vec<u8>>, String>;

/// What putting files back did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RestoreReport {
    /// Files written back or removed
    pub restored: Vec<PathBuf>,
    /// Files that could not be put back, with why
    pub failed: Vec<(PathBuf, String)>,
}

/// Per-turn file checkpoints of every agent in one terminal.
#[derive(Debug, Clone)]
pub struct FileCheckpoints {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    root: PathBuf,
    /// The turn each agent is running, if any
    open: Mutex<HashMap<AgentId, TurnManifest>>,
    /// The files each watched attempt changed, with what they held before
    attempts: Mutex<HashMap<AgentId, AttemptFiles>>,
}

/// The files one attempt changed, each with what it held before (`None`
/// when it did not exist), in the order they were first changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttemptFiles(Vec<(PathBuf, Option<Vec<u8>>)>);

impl FileCheckpoints {
    /// Checkpoints under `<data_dir>/checkpoints`.
    #[must_use]
    pub fn new(data_dir: &Path) -> Self {
        Self {
            inner: Arc::new(Inner {
                root: data_dir.join("checkpoints"),
                open: Mutex::new(HashMap::new()),
                attempts: Mutex::new(HashMap::new()),
            }),
        }
    }

    fn turns_dir(&self, agent: AgentId) -> PathBuf {
        self.inner.root.join(agent.to_hex()).join("turns")
    }

    fn blobs(&self, agent: AgentId) -> DirSnapshotStore {
        DirSnapshotStore::new(self.inner.root.join(agent.to_hex()).join("blobs"))
    }

    fn manifest_path(&self, agent: AgentId, started_ms: u64) -> PathBuf {
        self.turns_dir(agent).join(format!("{started_ms}.json"))
    }

    /// Start checkpointing the turn `agent` runs for `prompt`, whose message
    /// is at `message_index` in the session started at `session_started_ms`.
    pub fn begin_turn(
        &self,
        agent: AgentId,
        session_started_ms: u64,
        message_index: usize,
        prompt: &str,
    ) {
        let mut started_ms = aura_core_types::time::now_ms();
        // Manifests are named by start time; two turns in the same
        // millisecond (tests, instant replies) must not share a file.
        while self.manifest_path(agent, started_ms).exists() {
            started_ms += 1;
        }
        let manifest = TurnManifest {
            version: VERSION,
            session_started_ms,
            started_ms,
            message_index,
            prompt: preview(prompt),
            prompt_hash: hash(prompt.as_bytes()),
            files: Vec::new(),
            rewound: false,
            prompt_tx: None,
        };
        let mut open = lock(&self.inner.open);
        self.save_manifest(agent, &manifest);
        open.insert(agent, manifest);
    }

    /// Note that the prompt of the turn `agent` is running was recorded as
    /// the transaction `prompt_tx`.
    pub fn note_prompt_tx(&self, agent: AgentId, prompt_tx: Hash) {
        let mut open = lock(&self.inner.open);
        let Some(turn) = open.get_mut(&agent) else {
            return;
        };
        turn.prompt_tx = Some(prompt_tx.to_hex());
        let manifest = turn.clone();
        self.save_manifest(agent, &manifest);
    }

    /// Stop attributing file changes to `agent`'s turn.
    pub fn end_turn(&self, agent: AgentId) {
        lock(&self.inner.open).remove(&agent);
    }

    /// Keep what each file held before `agent`, or an agent it spawns,
    /// first changes it, until [`Self::take_attempt`].
    pub fn begin_attempt(&self, agent: AgentId) {
        lock(&self.inner.attempts).insert(agent, AttemptFiles::default());
    }

    /// Stop watching `agent`'s attempt and return the files it changed.
    #[must_use]
    pub fn take_attempt(&self, agent: AgentId) -> AttemptFiles {
        lock(&self.inner.attempts)
            .remove(&agent)
            .unwrap_or_default()
    }

    /// The files `agent`'s watched attempt changed so far; the attempt
    /// stays watched.
    #[must_use]
    pub fn attempt(&self, agent: AgentId) -> AttemptFiles {
        lock(&self.inner.attempts)
            .get(&agent)
            .cloned()
            .unwrap_or_default()
    }

    /// The session of the turn `agent` is running, if it is running one.
    #[must_use]
    pub fn open_session(&self, agent: AgentId) -> Option<u64> {
        lock(&self.inner.open)
            .get(&agent)
            .map(|turn| turn.session_started_ms)
    }

    /// A hook for the executor of the first agent in `chain`, which lists
    /// that agent and then the agents that spawned it, nearest first.
    /// Changes go to the turn of the first agent in `chain` running one,
    /// so a subagent's edits belong to the turn that started it. With
    /// `commands`, each change is also shown as a diff in its tool block.
    #[must_use]
    pub fn hook(
        &self,
        chain: Vec<AgentId>,
        commands: Option<mpsc::Sender<UiCommand>>,
    ) -> Arc<dyn FileChangeHook> {
        Arc::new(CheckpointHook {
            files: self.clone(),
            chain,
            commands,
            before: Mutex::new(HashMap::new()),
        })
    }

    /// Save what `path` held (`content`, `None` when absent) for the open
    /// turn of the first agent in `chain` that has one, unless that turn
    /// already saved it.
    pub fn note_before(&self, chain: &[AgentId], path: &Path, content: Option<&[u8]>) {
        for agent in chain {
            if let Some(files) = lock(&self.inner.attempts).get_mut(agent) {
                if !files.0.iter().any(|(seen, _)| seen == path) {
                    files
                        .0
                        .push((path.to_path_buf(), content.map(<[u8]>::to_vec)));
                }
            }
        }
        let owner = {
            let open = lock(&self.inner.open);
            let Some(owner) = chain.iter().copied().find(|a| open.contains_key(a)) else {
                return;
            };
            if open[&owner].files.iter().any(|f| f.path == path) {
                return;
            }
            owner
        };
        let blob = match content {
            Some(bytes) => {
                let key = hash(bytes);
                if let Err(e) = self
                    .blobs(owner)
                    .put(&key, bytes::Bytes::copy_from_slice(bytes))
                {
                    warn!(path = %path.display(), error = %e, "Could not checkpoint file");
                    return;
                }
                Some(key)
            }
            None => None,
        };
        let mut open = lock(&self.inner.open);
        let Some(turn) = open.get_mut(&owner) else {
            return;
        };
        if turn.files.iter().any(|f| f.path == path) {
            return;
        }
        turn.files.push(FileBefore {
            path: path.to_path_buf(),
            blob,
        });
        let manifest = turn.clone();
        self.save_manifest(owner, &manifest);
    }

    /// Write `manifest` whole through a temporary file. A failure is logged:
    /// the turn goes on, it just can't be rewound.
    fn save_manifest(&self, agent: AgentId, manifest: &TurnManifest) {
        let path = self.manifest_path(agent, manifest.started_ms);
        let written = serde_json::to_vec_pretty(manifest)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                std::fs::create_dir_all(self.turns_dir(agent))?;
                let tmp = path.with_extension("tmp");
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, &path)
            });
        if let Err(e) = written {
            warn!(path = %path.display(), error = %e, "Could not save turn checkpoint");
        }
    }

    /// Every turn of the session started at `session_started_ms`, rewound
    /// ones included, oldest first.
    #[must_use]
    pub fn session_manifests(&self, agent: AgentId, session_started_ms: u64) -> Vec<TurnManifest> {
        let Ok(entries) = std::fs::read_dir(self.turns_dir(agent)) else {
            return Vec::new();
        };
        let mut manifests: Vec<TurnManifest> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .filter_map(|path| {
                let bytes = std::fs::read(&path).ok()?;
                match serde_json::from_slice::<TurnManifest>(&bytes) {
                    Ok(manifest) if manifest.version == VERSION => Some(manifest),
                    Ok(_) => None,
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "Unreadable turn checkpoint");
                        None
                    }
                }
            })
            .filter(|m| m.session_started_ms == session_started_ms)
            .collect();
        manifests.sort_by_key(|m| m.started_ms);
        manifests
    }

    /// The turns of the session still in its conversation, numbered from
    /// 1, oldest first.
    #[must_use]
    pub fn turns(&self, agent: AgentId, session_started_ms: u64) -> Vec<Turn> {
        self.session_manifests(agent, session_started_ms)
            .into_iter()
            .filter(|m| !m.rewound)
            .enumerate()
            .map(|(i, manifest)| Turn {
                number: i + 1,
                manifest,
            })
            .collect()
    }

    /// Put every file `turns` changed back as it was before the earliest
    /// of them changed it: written back, or removed if it didn't exist.
    #[must_use]
    pub fn restore(&self, agent: AgentId, turns: &[Turn]) -> RestoreReport {
        let blobs = self.blobs(agent);
        let mut report = RestoreReport::default();
        for (path, blob) in earliest(turns.iter().map(|t| &t.manifest)) {
            let restored = match blob {
                Some(key) => match blobs.get(key) {
                    Ok(Some(bytes)) => write_file(path, &bytes).map_err(|e| e.to_string()),
                    Ok(None) => Err("its checkpoint is missing".to_string()),
                    Err(e) => Err(e.to_string()),
                },
                None => match std::fs::remove_file(path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                    _ => Ok(()),
                },
            };
            match restored {
                Ok(()) => report.restored.push(path.clone()),
                Err(why) => report.failed.push((path.clone(), why)),
            }
        }
        report
    }

    /// What each file `manifests` changed held before the earliest of them
    /// changed it.
    #[must_use]
    pub fn before<'a>(
        &self,
        agent: AgentId,
        manifests: impl IntoIterator<Item = &'a TurnManifest>,
    ) -> Vec<(PathBuf, Held)> {
        let blobs = self.blobs(agent);
        earliest(manifests)
            .into_iter()
            .map(|(path, blob)| {
                let content = match blob {
                    Some(key) => match blobs.get(key) {
                        Ok(Some(bytes)) => Ok(Some(bytes.to_vec())),
                        Ok(None) => Err("its checkpoint is missing".to_string()),
                        Err(e) => Err(e.to_string()),
                    },
                    None => Ok(None),
                };
                (path.clone(), content)
            })
            .collect()
    }

    /// Mark `turns` as no longer in the conversation. Their checkpoints stay
    /// on disk so `/diff session` still sees what they changed.
    pub fn mark_rewound(&self, agent: AgentId, turns: &[Turn]) {
        for turn in turns {
            let mut manifest = turn.manifest.clone();
            manifest.rewound = true;
            self.save_manifest(agent, &manifest);
        }
    }

    /// How each file `manifests` changed differs now from before the
    /// earliest of them changed it. Files back in their old state are left
    /// out; paths are shown relative to `workspace` when inside it.
    #[must_use]
    pub fn diff<'a>(
        &self,
        agent: AgentId,
        manifests: impl IntoIterator<Item = &'a TurnManifest>,
        workspace: Option<&Path>,
    ) -> Vec<FileDiffData> {
        let blobs = self.blobs(agent);
        let workspace = workspace.map(|w| w.canonicalize().unwrap_or_else(|_| w.to_path_buf()));
        let mut out = Vec::new();
        for (path, blob) in earliest(manifests) {
            let before = match blob {
                Some(key) => match blobs.get(key) {
                    Ok(Some(bytes)) => Some(bytes.to_vec()),
                    Ok(None) | Err(_) => {
                        out.push(FileDiffData {
                            path: display_path(path, workspace.as_deref()),
                            diff: "(its checkpoint is missing; no diff)".to_string(),
                        });
                        continue;
                    }
                },
                None => None,
            };
            let now = match read_optional(path) {
                Ok(now) => now,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "Could not read file for diff");
                    continue;
                }
            };
            if before == now {
                continue;
            }
            let diff = unified_diff(before.as_deref(), now.as_deref())
                .unwrap_or_else(|| "(binary or very large file changed; no diff)".to_string());
            out.push(FileDiffData {
                path: display_path(path, workspace.as_deref()),
                diff,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests;
