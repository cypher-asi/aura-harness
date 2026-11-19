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
use aura_ui_terminal::events::FileDiffData;
use aura_ui_terminal::UiCommand;
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::sync::mpsc;
use tracing::warn;

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

impl AttemptFiles {
    /// The files, in the order they were first changed.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.0.iter().map(|(path, _)| path.as_path())
    }

    /// Whether the attempt changed no file.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How the files differ now from before the attempt, as one unified
    /// diff per file under its path (relative to `workspace` when inside
    /// it). Files back in their old state are left out; empty when
    /// nothing differs.
    #[must_use]
    pub fn diff(&self, workspace: Option<&Path>) -> String {
        let workspace = workspace.map(|w| w.canonicalize().unwrap_or_else(|_| w.to_path_buf()));
        let mut out = String::new();
        for (path, before) in &self.0 {
            let Ok(now) = read_optional(path) else {
                continue;
            };
            if *before == now {
                continue;
            }
            let diff = unified_diff(before.as_deref(), now.as_deref())
                .unwrap_or_else(|| "(binary or very large file changed; no diff)\n".to_string());
            out.push_str("=== ");
            out.push_str(&display_path(path, workspace.as_deref()));
            out.push('\n');
            out.push_str(&diff);
            if !diff.ends_with('\n') {
                out.push('\n');
            }
        }
        out
    }

    /// Put every file back as it was before the attempt: written back, or
    /// removed if it didn't exist.
    #[must_use]
    pub fn restore(&self) -> RestoreReport {
        let mut report = RestoreReport::default();
        for (path, before) in &self.0 {
            let restored = match before {
                Some(bytes) => write_file(path, bytes),
                None => match std::fs::remove_file(path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                },
            };
            match restored {
                Ok(()) => report.restored.push(path.clone()),
                Err(e) => report.failed.push((path.clone(), e.to_string())),
            }
        }
        report
    }
}

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
        let mut started_ms = crate::session_checkpoint::now_ms();
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

/// Each file `manifests` changed, with what it held before the first of
/// them changed it, by path.
fn earliest<'a>(
    manifests: impl IntoIterator<Item = &'a TurnManifest>,
) -> BTreeMap<&'a PathBuf, &'a Option<String>> {
    let mut sorted: Vec<&TurnManifest> = manifests.into_iter().collect();
    sorted.sort_by_key(|m| m.started_ms);
    let mut out = BTreeMap::new();
    for file in sorted.iter().flat_map(|m| &m.files) {
        out.entry(&file.path).or_insert(&file.blob);
    }
    out
}

/// Unified diff hunks from `before` to `after` (`None` for a missing file).
/// `None` when either side is not UTF-8 text or both are very large.
#[must_use]
pub fn unified_diff(before: Option<&[u8]>, after: Option<&[u8]>) -> Option<String> {
    let old = std::str::from_utf8(before.unwrap_or_default()).ok()?;
    let new = std::str::from_utf8(after.unwrap_or_default()).ok()?;
    if old.len() + new.len() > MAX_DIFF_BYTES {
        return None;
    }
    Some(
        TextDiff::from_lines(old, new)
            .unified_diff()
            .context_radius(3)
            .to_string(),
    )
}

/// The file at `path`, or `None` when there is none.
fn read_optional(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)
}

fn display_path(path: &Path, workspace: Option<&Path>) -> String {
    workspace
        .and_then(|w| path.strip_prefix(w).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// The first line of `prompt`, cut to [`PROMPT_CHARS`].
fn preview(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or("").trim();
    let mut out: String = line.chars().take(PROMPT_CHARS).collect();
    if line.chars().count() > PROMPT_CHARS || prompt.trim().lines().nth(1).is_some() {
        out.push('…');
    }
    out
}

/// A file-changing call in flight: its tool use id and the file.
type CallKey = (String, PathBuf);

/// Checkpoints files for one agent's executor and shows each change in
/// its tool block.
struct CheckpointHook {
    files: FileCheckpoints,
    chain: Vec<AgentId>,
    commands: Option<mpsc::Sender<UiCommand>>,
    /// Content before each call in flight, for its diff
    before: Mutex<HashMap<CallKey, Option<Vec<u8>>>>,
}

#[async_trait]
impl FileChangeHook for CheckpointHook {
    async fn before_change(&self, path: &Path, tool_use_id: Option<&str>) {
        let files = self.files.clone();
        let chain = self.chain.clone();
        let target = path.to_path_buf();
        let read = tokio::task::spawn_blocking(move || {
            let content = read_optional(&target)?;
            files.note_before(&chain, &target, content.as_deref());
            Ok::<_, std::io::Error>(content)
        })
        .await;
        let content = match read {
            Ok(Ok(content)) => content,
            Ok(Err(e)) => {
                warn!(path = %path.display(), error = %e, "Could not read file before a change");
                return;
            }
            Err(e) => {
                warn!(error = %e, "File checkpoint task failed");
                return;
            }
        };
        if let (Some(_), Some(id)) = (&self.commands, tool_use_id) {
            lock(&self.before).insert((id.to_string(), path.to_path_buf()), content);
        }
    }

    async fn after_change(&self, path: &Path, tool_use_id: Option<&str>, succeeded: bool) {
        let (Some(commands), Some(id)) = (&self.commands, tool_use_id) else {
            return;
        };
        let Some(before) = lock(&self.before).remove(&(id.to_string(), path.to_path_buf())) else {
            return;
        };
        if !succeeded {
            return;
        }
        let target = path.to_path_buf();
        let diff = tokio::task::spawn_blocking(move || {
            let after = read_optional(&target).ok()?;
            unified_diff(before.as_deref(), after.as_deref())
        })
        .await
        .ok()
        .flatten()
        .filter(|diff| !diff.is_empty());
        let (Some(diff), Some(agent)) = (diff, self.chain.first()) else {
            return;
        };
        AgentUi::new(commands.clone(), *agent)
            .send(UiCommand::ToolDiff {
                id: id.to_string(),
                diff,
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, FileCheckpoints, AgentId, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let files = FileCheckpoints::new(&dir.path().join("data"));
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        (dir, files, AgentId::generate(), workspace)
    }

    /// Change `path` to `content` (`None` deletes it) as a tool would.
    async fn change(hook: &Arc<dyn FileChangeHook>, path: &Path, content: Option<&str>) {
        hook.before_change(path, Some("t1")).await;
        match content {
            Some(text) => std::fs::write(path, text).unwrap(),
            None => std::fs::remove_file(path).unwrap(),
        }
        hook.after_change(path, Some("t1"), true).await;
    }

    #[tokio::test]
    async fn rewind_puts_files_back_as_they_were_before_the_turn() {
        let (_dir, files, agent, ws) = setup();
        let hook = files.hook(vec![agent], None);
        let kept = ws.join("kept.txt");
        let created = ws.join("new/created.txt");
        std::fs::write(&kept, "original\n").unwrap();

        files.begin_turn(agent, 1, 0, "first");
        change(&hook, &kept, Some("turn one\n")).await;
        files.end_turn(agent);
        files.begin_turn(agent, 1, 2, "second");
        change(&hook, &kept, Some("turn two\n")).await;
        std::fs::create_dir_all(created.parent().unwrap()).unwrap();
        change(&hook, &created, Some("hello\n")).await;
        files.end_turn(agent);

        let turns = files.turns(agent, 1);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[1].manifest.files.len(), 2);
        assert_eq!(
            turns[1].manifest.files[1].blob, None,
            "created file was absent"
        );

        let report = files.restore(agent, &turns[1..]);
        assert!(report.failed.is_empty(), "{report:?}");
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "turn one\n");
        assert!(!created.exists());

        let report = files.restore(agent, &turns);
        assert!(report.failed.is_empty(), "{report:?}");
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "original\n");
    }

    #[tokio::test]
    async fn an_attempt_and_its_subagents_changes_can_be_put_back() {
        let (_dir, files, attempt, ws) = setup();
        let nested = AgentId::generate();
        let outsider = files.hook(vec![AgentId::generate()], None);
        let kept = ws.join("kept.txt");
        let created = ws.join("created.txt");
        let other = ws.join("other.txt");
        std::fs::write(&kept, "original\n").unwrap();

        files.begin_attempt(attempt);
        change(&files.hook(vec![attempt], None), &kept, Some("one\n")).await;
        change(
            &files.hook(vec![nested, attempt], None),
            &kept,
            Some("two\n"),
        )
        .await;
        change(
            &files.hook(vec![nested, attempt], None),
            &created,
            Some("new\n"),
        )
        .await;
        change(&outsider, &other, Some("not ours\n")).await;
        let changed = files.take_attempt(attempt);
        assert_eq!(
            changed.paths().collect::<Vec<_>>(),
            [kept.as_path(), created.as_path()]
        );

        let report = changed.restore();
        assert!(report.failed.is_empty(), "{report:?}");
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "original\n");
        assert!(!created.exists());
        assert!(other.exists(), "files of other work are left alone");
        assert!(files.take_attempt(attempt).is_empty());
    }

    #[test]
    fn a_turn_keeps_the_record_hash_of_its_prompt() {
        let (_dir, files, agent, _ws) = setup();
        let prompt_tx = Hash::from_content(b"prompt");
        files.note_prompt_tx(agent, prompt_tx);
        files.begin_turn(agent, 1, 0, "first");
        files.note_prompt_tx(agent, prompt_tx);
        files.end_turn(agent);
        files.note_prompt_tx(agent, Hash::from_content(b"later"));

        let turns = files.turns(agent, 1);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].manifest.prompt_tx, Some(prompt_tx.to_hex()));
    }

    #[tokio::test]
    async fn only_the_first_change_in_a_turn_is_saved_and_deletes_come_back() {
        let (_dir, files, agent, ws) = setup();
        let hook = files.hook(vec![agent], None);
        let doomed = ws.join("doomed.txt");
        std::fs::write(&doomed, "keep me\n").unwrap();

        files.begin_turn(agent, 1, 0, "clean up");
        change(&hook, &doomed, Some("edited\n")).await;
        change(&hook, &doomed, None).await;
        files.end_turn(agent);

        let turns = files.turns(agent, 1);
        assert_eq!(turns[0].manifest.files.len(), 1);
        let report = files.restore(agent, &turns);
        assert_eq!(report.restored, std::slice::from_ref(&doomed));
        assert_eq!(std::fs::read_to_string(&doomed).unwrap(), "keep me\n");
    }

    #[tokio::test]
    async fn changes_outside_a_turn_go_to_the_spawning_agents_turn_or_nowhere() {
        let (_dir, files, parent, ws) = setup();
        let child = AgentId::generate();
        let hook = files.hook(vec![child, parent], None);
        let path = ws.join("a.txt");

        change(&hook, &path, Some("no turn\n")).await;
        assert!(files.turns(parent, 1).is_empty());

        files.begin_turn(parent, 1, 0, "delegate");
        change(&hook, &path, Some("by the child\n")).await;
        files.end_turn(parent);
        let turns = files.turns(parent, 1);
        assert_eq!(turns[0].manifest.files.len(), 1);
        assert!(files.turns(child, 1).is_empty());
    }

    #[tokio::test]
    async fn diff_compares_now_with_before_the_turns() {
        let (_dir, files, agent, ws) = setup();
        let hook = files.hook(vec![agent], None);
        let a = ws.join("a.txt");
        let b = ws.join("b.txt");
        std::fs::write(&a, "one\ntwo\n").unwrap();
        std::fs::write(&b, "same\n").unwrap();

        files.begin_turn(agent, 1, 0, "edit");
        change(&hook, &a, Some("one\nTWO\n")).await;
        change(&hook, &b, Some("changed\n")).await;
        std::fs::write(&b, "same\n").unwrap();
        files.end_turn(agent);

        let manifests = files.session_manifests(agent, 1);
        let diffs = files.diff(agent, &manifests, Some(&ws));
        assert_eq!(diffs.len(), 1, "b.txt is back as it was: {diffs:?}");
        assert_eq!(diffs[0].path, "a.txt");
        assert!(diffs[0].diff.contains("-two\n+TWO\n"), "{}", diffs[0].diff);
    }

    #[tokio::test]
    async fn rewound_turns_drop_out_of_the_list_but_not_the_session() {
        let (_dir, files, agent, _ws) = setup();
        files.begin_turn(agent, 1, 0, "one");
        files.end_turn(agent);
        files.begin_turn(agent, 1, 2, "two\nmore detail");
        files.end_turn(agent);
        files.begin_turn(agent, 9, 0, "other session");
        files.end_turn(agent);

        let turns = files.turns(agent, 1);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[1].manifest.prompt, "two…");
        assert!(turns[1].manifest.is_prompt("two\nmore detail"));

        files.mark_rewound(agent, &turns[1..]);
        assert_eq!(files.turns(agent, 1).len(), 1);
        assert_eq!(files.session_manifests(agent, 1).len(), 2);
    }

    #[tokio::test]
    async fn the_hook_shows_each_change_in_its_tool_block() {
        let (_dir, files, agent, ws) = setup();
        let (tx, mut rx) = mpsc::channel(8);
        let hook = files.hook(vec![agent], Some(tx));
        let path = ws.join("x.txt");
        std::fs::write(&path, "a\n").unwrap();

        change(&hook, &path, Some("b\n")).await;

        let Some(UiCommand::ForAgent { agent_id, command }) = rx.recv().await else {
            panic!("expected a command for the agent's chat");
        };
        assert_eq!(agent_id, agent.to_hex());
        let UiCommand::ToolDiff { id, diff } = *command else {
            panic!("expected a tool diff");
        };
        assert_eq!(id, "t1");
        assert!(diff.contains("-a\n+b\n"), "{diff}");
    }
}
