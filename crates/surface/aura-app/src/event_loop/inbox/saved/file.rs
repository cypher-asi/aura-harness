//! The file the inbox is kept in between runs: what it holds, read
//! within bounds, and replaced atomically.
//!
//! # Failure modes
//!
//! A missing file is an empty inbox. A file that can't be read, is
//! larger than [`MAX_BYTES`], isn't JSON of this shape or has another
//! [`VERSION`] is an empty inbox too, with a warning; the next save
//! replaces it. A write goes to a temporary file beside the real one,
//! is synced, then renamed over it, so a crash leaves the old file or
//! the new one, never half of either.

pub(super) use crate::event_loop::factory::saved::{SavedFlow, MAX_FLOWS};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tracing::warn;

/// The file's name, in the app's data directory.
pub(crate) const SAVED_FILE: &str = "inbox-owed.json";
/// The shape of the file this build reads and writes.
pub(super) const VERSION: u32 = 1;
/// Largest file read; a larger one counts as corrupt.
pub(super) const MAX_BYTES: u64 = 8 * 1024 * 1024;
/// Most owed reports kept, the oldest first.
pub(super) const MAX_OWED: usize = 1_000;
/// Most queued messages kept, the oldest first.
pub(super) const MAX_QUEUED: usize = 200;
/// Most undelivered messages kept, the oldest first.
pub(super) const MAX_NOTICES: usize = 200;

/// What an inbox message is, as the file says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum SavedKind {
    Assignment,
    Request,
    Reply,
}

/// What a queued message's turn is on its agent's task board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum SavedRequest {
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
}

/// An inbox message waiting in `agent`'s queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SavedQueued {
    pub agent: String,
    pub text: String,
    pub request: SavedRequest,
}

/// An inbox message sent but not yet given to its agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SavedNotice {
    pub to: String,
    pub shown: String,
    pub prompt: String,
    pub kind: SavedKind,
}

/// Everything the file holds. `flows`, the software factory's stages,
/// came later: a file without them has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Snapshot {
    pub version: u32,
    pub owed: Vec<SavedOwed>,
    pub queued: Vec<SavedQueued>,
    pub notices: Vec<SavedNotice>,
    #[serde(default)]
    pub flows: Vec<SavedFlow>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: VERSION,
            owed: Vec::new(),
            queued: Vec::new(),
            notices: Vec::new(),
            flows: Vec::new(),
        }
    }
}

impl Snapshot {
    /// The snapshot within [`MAX_OWED`], [`MAX_QUEUED`] and
    /// [`MAX_NOTICES`], keeping the oldest of each. Warns when it cuts.
    pub(super) fn capped(mut self) -> Self {
        let lengths = |s: &Self| (s.owed.len(), s.queued.len(), s.notices.len(), s.flows.len());
        let before = lengths(&self);
        self.owed.truncate(MAX_OWED);
        self.queued.truncate(MAX_QUEUED);
        self.notices.truncate(MAX_NOTICES);
        self.flows.truncate(MAX_FLOWS);
        if before != lengths(&self) {
            warn!(
                owed = before.0,
                queued = before.1,
                notices = before.2,
                flows = before.3,
                "The saved inbox is over its limits; the newest entries are not kept"
            );
        }
        self
    }
}

/// The snapshot `path` holds: empty when it is missing, and, with a
/// warning, when it is unreadable, too large, corrupt or of another
/// version.
pub(super) fn load(path: &Path) -> Snapshot {
    match read(path) {
        Ok(Some(snapshot)) => snapshot.capped(),
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
    if snapshot.version != VERSION {
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
