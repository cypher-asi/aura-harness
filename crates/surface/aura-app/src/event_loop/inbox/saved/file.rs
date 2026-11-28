//! The file the inbox is kept in between runs: what it holds, read
//! within bounds, and replaced atomically.
//!
//! Version 2 adds what was running, every kind of queued work, the
//! inbox's threads and parked agents to version 1 (#85), which still
//! reads: its fields come back and the new ones are empty.
//!
//! # Failure modes
//!
//! A missing file is an empty inbox. A file that can't be read, is
//! larger than [`MAX_BYTES`], isn't JSON of this shape or has an unknown
//! version is an empty inbox too, with a warning; the next save replaces
//! it. A write goes to a temporary file beside the real one, is synced,
//! then renamed over it, so a crash leaves the old file or the new one,
//! never half of either.

pub(super) use crate::event_loop::factory::saved::{SavedFlow, MAX_FLOWS};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tracing::warn;

/// The file's name, in the app's data directory.
pub(crate) const SAVED_FILE: &str = "inbox-owed.json";
/// The shape of the file this build writes.
pub(super) const VERSION: u32 = 2;
/// The older shape this build still reads.
const VERSION_1: u32 = 1;
/// Largest file read; a larger one counts as corrupt.
pub(super) const MAX_BYTES: u64 = 8 * 1024 * 1024;
/// Most owed reports kept, the oldest first.
pub(super) const MAX_OWED: usize = 1_000;
/// Most queued items kept, the oldest first.
pub(super) const MAX_QUEUED: usize = 200;
/// Most undelivered messages kept, the oldest first.
pub(super) const MAX_NOTICES: usize = 200;
/// Most threads, subjects and reports kept each.
pub(super) const MAX_THREADS: usize = 2_000;

/// What an inbox message is, as the file says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum SavedKind {
    Assignment,
    Request,
    Reply,
}

/// What a queued message's turn is on its agent's task board.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum SavedRequest {
    #[default]
    New,
    Build,
    Resume,
}

/// A report `agent` owes `from`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SavedOwed {
    /// Hex id of the agent that owes it
    pub agent: String,
    pub id: String,
    pub kind: SavedKind,
    /// Hex id of the agent it goes back to
    pub from: String,
    pub thread: String,
    pub subject: String,
    /// The sender's todo it closes
    pub todo: Option<String>,
    /// Taken back before it started: it must never start
    pub withdrawn: bool,
    /// A turn started on it: the turn that ends its work answers it.
    /// Absent in files written before it existed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub taken: bool,
}

/// What kind of work a queued item is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum SavedWorkKind {
    /// A message for the model (all version 1 kept)
    #[default]
    Prompt,
    /// A shell command typed in the chat
    Command,
    /// `/compact`
    Compact,
    /// `/build`
    Build,
}

/// An image pasted into a queued message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SavedImage {
    pub media_type: String,
    /// Base64, as pasted
    pub data: String,
    pub bytes: usize,
}

/// Work waiting in `agent`'s queue. Version 1 kept only inbox messages
/// (`agent`, `text`, `request`); the other fields default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SavedQueued {
    pub agent: String,
    #[serde(default)]
    pub work: SavedWorkKind,
    /// A prompt's text, or a command
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub request: SavedRequest,
    /// Whether `@path` words in a prompt attach files
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mentions: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preamble: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<SavedImage>,
    /// Images too large to keep, left out
    #[serde(default, skip_serializing_if = "is_zero")]
    pub images_dropped: usize,
    /// A command's terminal width
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub cols: u16,
    /// `/compact`'s focus or `/build`'s path
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument: Option<String>,
}

const fn is_zero(n: &usize) -> bool {
    *n == 0
}

const fn is_zero_u16(n: &u16) -> bool {
    *n == 0
}

/// An inbox message sent but not yet given to its agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SavedNotice {
    pub to: String,
    pub shown: String,
    pub prompt: String,
    pub kind: SavedKind,
}

/// A turn under way at the last save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SavedRunning {
    pub agent: String,
    /// On a cloud machine, not here: the cloud hub's own turn mark
    /// follows it ([`crate::cloud::CloudHub::follow_turn`])
    #[serde(default)]
    pub cloud: bool,
}

/// What the inbox's threads carried.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SavedThreads {
    /// Requests each thread carried
    #[serde(default)]
    pub requests: Vec<(String, u32)>,
    /// The subject each two agents last wrote about: (low hex, high hex,
    /// subject)
    #[serde(default)]
    pub subjects: Vec<(String, String, String)>,
    /// Each agent's last reply: whether it finished, and what it said
    #[serde(default)]
    pub reports: Vec<(String, bool, String)>,
}

/// Everything the file holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Snapshot {
    pub version: u32,
    pub owed: Vec<SavedOwed>,
    pub queued: Vec<SavedQueued>,
    /// Version 1 called these `notices`, kept only at quit
    #[serde(alias = "notices")]
    pub outbox: Vec<SavedNotice>,
    #[serde(default)]
    pub running: Vec<SavedRunning>,
    #[serde(default)]
    pub threads: SavedThreads,
    /// Hex ids of the agents the user stopped to carry on
    #[serde(default)]
    pub parked: Vec<String>,
    /// The software factory's stages ([`crate::event_loop::factory`])
    #[serde(default)]
    pub flows: Vec<SavedFlow>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: VERSION,
            owed: Vec::new(),
            queued: Vec::new(),
            outbox: Vec::new(),
            running: Vec::new(),
            threads: SavedThreads::default(),
            parked: Vec::new(),
            flows: Vec::new(),
        }
    }
}

impl Snapshot {
    /// The snapshot within its limits, keeping the oldest of each.
    /// Warns when it cuts.
    pub(super) fn capped(mut self) -> Self {
        let before = self.sizes();
        self.owed.truncate(MAX_OWED);
        self.queued.truncate(MAX_QUEUED);
        self.outbox.truncate(MAX_NOTICES);
        self.threads.requests.truncate(MAX_THREADS);
        self.threads.subjects.truncate(MAX_THREADS);
        self.threads.reports.truncate(MAX_THREADS);
        self.flows.truncate(MAX_FLOWS);
        if before != self.sizes() {
            warn!(
                owed = before[0],
                queued = before[1],
                outbox = before[2],
                "The saved inbox is over its limits; the newest entries are not kept"
            );
        }
        self
    }

    fn sizes(&self) -> [usize; 7] {
        [
            self.owed.len(),
            self.queued.len(),
            self.outbox.len(),
            self.threads.requests.len(),
            self.threads.subjects.len(),
            self.threads.reports.len(),
            self.flows.len(),
        ]
    }
}

/// The snapshot `path` holds: empty when it is missing, and, with a
/// warning, when it is unreadable, too large, corrupt or of an unknown
/// version. A version 1 file reads as version 2.
pub(super) fn load(path: &Path) -> Snapshot {
    match read(path) {
        Ok(Some(snapshot)) => Snapshot {
            version: VERSION,
            ..snapshot
        }
        .capped(),
        Ok(None) => Snapshot::default(),
        Err(why) => {
            warn!(
                path = %path.display(),
                error = %why,
                "The saved inbox can't be used; starting with an empty one"
            );
            Snapshot::default()
        }
    }
}

/// The snapshot in `path`, `None` when there is no file.
fn read(path: &Path) -> Result<Option<Snapshot>, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("opening it: {e}")),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("reading it: {e}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_BYTES {
        return Err(format!("it is larger than {MAX_BYTES} bytes"));
    }
    let snapshot: Snapshot =
        serde_json::from_slice(&bytes).map_err(|e| format!("parsing it: {e}"))?;
    if snapshot.version != VERSION && snapshot.version != VERSION_1 {
        return Err(format!("it is version {}, not {VERSION}", snapshot.version));
    }
    Ok(Some(snapshot))
}

/// Replace `path` with `snapshot`, atomically. Blocks: run it off the
/// async threads.
///
/// # Errors
/// When the snapshot can't be encoded, or the folder or the file can't
/// be written.
pub(super) fn save(path: &Path, snapshot: &Snapshot) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(snapshot).map_err(std::io::Error::other)?;
    write_atomically(path, &bytes)
}

/// Write `bytes` to a file beside `path`, sync it, then rename it over
/// `path`.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let tmp = temporary(path);
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

/// The temporary file a write of `path` goes to first.
fn temporary(path: &Path) -> PathBuf {
    path.with_extension("json.tmp")
}
