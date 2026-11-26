//! Notes in an agent's chat that its record does not hold: the boot
//! consoles of its moves to and from the cloud and of its machine waking,
//! and the errors, warnings and system lines sent to its chat.
//!
//! The record can't take them: a moving or cloud agent's record has its
//! machine as its one writer. So they go to
//! `<data_dir>/sessions/<agent>/notes.jsonl` as they are sent, and a chat
//! rebuilt from the record (on a restart, or on switching to an agent whose
//! chat the UI doesn't hold) has them put back among its messages by time.
//!
//! A boot console is kept as its updates. Its log lines wait in memory and
//! are written, the newest [`KEPT_LOG_LINES`] of them, before its next
//! step changes and when it ends; progress is not kept. A console that
//! never ended (the app closed during it) shows as stopped, unless it is
//! still running in this process.

use crate::session_checkpoint::now_ms;
use aura_core_types::AgentId;
use aura_ui_terminal::events::{BootHistory, BootLogLine, BootUpdate, MessageData, MessageRole};
use aura_ui_terminal::UiCommand;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tracing::warn;

/// Name of an agent's notes file in its sessions folder.
const FILE: &str = "notes.jsonl";

/// Log lines of a console kept from one write to the next.
const KEPT_LOG_LINES: usize = 500;

/// Notes put back in a rebuilt chat, the newest.
const MAX_NOTES: usize = 100;

/// Notes a file may hold before it is cut back to the newest
/// [`MAX_NOTES`].
const COMPACT_AFTER: usize = 2 * MAX_NOTES;

/// How a console that never ended is shown once nothing runs it.
const INTERRUPTED: &str = "Stopped: Aura closed before this finished.";

/// One line of a notes file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    ts_ms: u64,
    #[serde(flatten)]
    note: Note,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Note {
    /// A system line, as the chat shows it
    Notice { text: String },
    /// An update of boot console `id`
    Boot { id: String, update: BootUpdate },
}

/// The log lines of a running console not written yet.
#[derive(Debug, Default)]
struct OpenConsole {
    lines: VecDeque<BootLogLine>,
    /// Lines that came and went before being written
    dropped: usize,
}

impl OpenConsole {
    fn keep(&mut self, lines: &[BootLogLine]) {
        self.lines.extend(lines.iter().cloned());
        while self.lines.len() > KEPT_LOG_LINES {
            self.lines.pop_front();
            self.dropped += 1;
        }
    }

    /// The lines waiting, with a line saying how many were not kept.
    fn pending(&self) -> Option<Vec<BootLogLine>> {
        if self.lines.is_empty() && self.dropped == 0 {
            return None;
        }
        let skipped = (self.dropped > 0).then(|| BootLogLine {
            stderr: false,
            text: format!("… {} earlier lines not kept", self.dropped),
        });
        Some(
            skipped
                .into_iter()
                .chain(self.lines.iter().cloned())
                .collect(),
        )
    }

    fn take(&mut self) -> Option<Vec<BootLogLine>> {
        let pending = self.pending();
        *self = Self::default();
        pending
    }
}

/// Where the notes of every agent's chat are kept.
#[derive(Debug)]
pub struct ChatNotes {
    root: PathBuf,
    /// Consoles running in this process, by id. Its lock also keeps
    /// writes to the files one at a time.
    open: Mutex<HashMap<String, OpenConsole>>,
}

impl ChatNotes {
    /// Notes under `<data_dir>/sessions`, beside the session checkpoints.
    #[must_use]
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("sessions"),
            open: Mutex::default(),
        }
    }

    fn file(&self, agent: AgentId) -> PathBuf {
        self.root.join(agent.to_hex()).join(FILE)
    }

    /// Keep what `command`, sent to `agent`'s chat, adds to it, if it is a
    /// note.
    pub fn record(&self, agent: AgentId, command: &UiCommand) {
        let mut open = self.open.lock();
        let notes = match command {
            UiCommand::Boot { id, update } => boot_notes(&mut open, id, update),
            _ => match command.chat_notice() {
                Some(text) => vec![Note::Notice { text }],
                None => return,
            },
        };
        if notes.is_empty() {
            return;
        }
        let ts_ms = now_ms();
        let entries: Vec<Entry> = notes
            .into_iter()
            .map(|note| Entry { ts_ms, note })
            .collect();
        if let Err(e) = append(&self.file(agent), &entries) {
            warn!(%agent, error = %e, "Failed to keep a chat note");
        }
    }

    /// `agent`'s notes as chat messages, oldest first, the newest
    /// [`MAX_NOTES`] of them.
    #[must_use]
    pub fn chat(&self, agent: AgentId) -> Vec<MessageData> {
        let open = self.open.lock();
        let path = self.file(agent);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        let entries: Vec<Entry> = text
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let mut folded = fold(&entries);
        if folded.len() > COMPACT_AFTER {
            folded.drain(..folded.len() - MAX_NOTES);
            if let Err(e) = rewrite(&path, &entries, &folded) {
                warn!(%agent, error = %e, "Failed to cut back the chat notes");
            }
        }
        let skip = folded.len().saturating_sub(MAX_NOTES);
        folded
            .into_iter()
            .skip(skip)
            .map(|note| note.into_message(&open))
            .collect()
    }
}

/// What a console's `update` adds to its notes. Log lines wait for the
/// next step or the end; progress is not kept.
fn boot_notes(open: &mut HashMap<String, OpenConsole>, id: &str, update: &BootUpdate) -> Vec<Note> {
    let console = open.entry(id.to_string()).or_default();
    let boot = |update| Note::Boot {
        id: id.to_string(),
        update,
    };
    match update {
        BootUpdate::Log(lines) => {
            console.keep(lines);
            Vec::new()
        }
        BootUpdate::Progress { .. } => Vec::new(),
        update => {
            let mut notes: Vec<Note> = console
                .take()
                .map(|lines| boot(BootUpdate::Log(lines)))
                .into_iter()
                .collect();
            notes.push(boot(update.clone()));
            if matches!(update, BootUpdate::End { .. }) {
                open.remove(id);
            }
            notes
        }
    }
}

fn append(path: &Path, entries: &[Entry]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut bytes = Vec::new();
    for entry in entries {
        serde_json::to_writer(&mut bytes, entry)?;
        bytes.push(b'\n');
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(&bytes)
}

/// Write the entries of `kept` alone to `path`, replacing it whole.
fn rewrite(path: &Path, entries: &[Entry], kept: &[Folded]) -> std::io::Result<()> {
    let keep: HashSet<usize> = kept
        .iter()
        .flat_map(|note| note.entries.iter().copied())
        .collect();
    let mut bytes = Vec::new();
    for (at, entry) in entries.iter().enumerate() {
        if keep.contains(&at) {
            serde_json::to_writer(&mut bytes, entry)?;
            bytes.push(b'\n');
        }
    }
    let partial = path.with_extension("jsonl.partial");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path)
}

/// A note of the chat: a notice, or a console with all its updates.
#[derive(Debug)]
struct Folded {
    first_ms: u64,
    last_ms: u64,
    kind: FoldedKind,
    /// Where its entries are in the file
    entries: Vec<usize>,
}

#[derive(Debug)]
enum FoldedKind {
    Notice(String),
    Boot {
        id: String,
        updates: Vec<BootUpdate>,
    },
}

/// `entries` as notes, in the order each began.
fn fold(entries: &[Entry]) -> Vec<Folded> {
    let mut folded: Vec<Folded> = Vec::new();
    let mut consoles: HashMap<&str, usize> = HashMap::new();
    for (at, entry) in entries.iter().enumerate() {
        match &entry.note {
            Note::Notice { text } => folded.push(Folded {
                first_ms: entry.ts_ms,
                last_ms: entry.ts_ms,
                kind: FoldedKind::Notice(text.clone()),
                entries: vec![at],
            }),
            Note::Boot { id, update } => {
                let index = *consoles.entry(id).or_insert_with(|| {
                    folded.push(Folded {
                        first_ms: entry.ts_ms,
                        last_ms: entry.ts_ms,
                        kind: FoldedKind::Boot {
                            id: id.clone(),
                            updates: Vec::new(),
                        },
                        entries: Vec::new(),
                    });
                    folded.len() - 1
                });
                let note = &mut folded[index];
                note.last_ms = entry.ts_ms;
                note.entries.push(at);
                if let FoldedKind::Boot { updates, .. } = &mut note.kind {
                    updates.push(update.clone());
                }
            }
        }
    }
    folded
}

impl Folded {
    /// The chat message of this note. A console that never ended goes on
    /// with the lines it has shown since its last write while it still
    /// runs in this process (`open`), and shows as stopped otherwise.
    fn into_message(self, open: &HashMap<String, OpenConsole>) -> MessageData {
        let at = Some(self.first_ms);
        match self.kind {
            FoldedKind::Notice(text) => MessageData {
                timestamp_ms: at,
                ..MessageData::new(MessageRole::System, text)
            },
            FoldedKind::Boot { id, mut updates } => {
                let ended = matches!(updates.last(), Some(BootUpdate::End { .. }));
                let running = open.get(&id).filter(|_| !ended);
                match running {
                    Some(console) => updates.extend(console.pending().map(BootUpdate::Log)),
                    None if !ended => updates.push(BootUpdate::End {
                        ok: false,
                        summary: INTERRUPTED.to_string(),
                    }),
                    None => {}
                }
                MessageData {
                    timestamp_ms: at,
                    boot: Some(BootHistory {
                        id,
                        updates,
                        elapsed_ms: running
                            .is_none()
                            .then(|| self.last_ms.saturating_sub(self.first_ms)),
                    }),
                    ..MessageData::new(MessageRole::System, "")
                }
            }
        }
    }
}

/// `history` with `notes` among it, each after the messages sent before
/// or with it, the newest `limit` kept.
#[must_use]
pub fn interleave(
    history: Vec<MessageData>,
    notes: Vec<MessageData>,
    limit: usize,
) -> Vec<MessageData> {
    let mut merged = Vec::with_capacity(history.len() + notes.len());
    let mut notes = notes.into_iter().peekable();
    for message in history {
        if let Some(at) = message.timestamp_ms {
            while let Some(note) = notes.next_if(|note| note.timestamp_ms.is_some_and(|t| t < at)) {
                merged.push(note);
            }
        }
        merged.push(message);
    }
    merged.extend(notes);
    let skip = merged.len().saturating_sub(limit);
    merged.into_iter().skip(skip).collect()
}

/// The notes of this process's session, once [`install`]ed.
static NOTES: OnceLock<ChatNotes> = OnceLock::new();

/// Keep the notes of every agent's chat in `notes` from now on.
pub fn install(notes: ChatNotes) {
    if NOTES.set(notes).is_err() {
        warn!("Chat notes were already being kept");
    }
}

/// Keep what `command` adds to `agent`'s chat, if it is a note and notes
/// are kept.
pub(crate) fn record(agent: AgentId, command: &UiCommand) {
    if let Some(notes) = NOTES.get() {
        notes.record(agent, command);
    }
}

/// `history`, `agent`'s chat rebuilt from its record, with its notes
/// among it when notes are kept.
#[must_use]
pub fn with_notes(agent: AgentId, history: Vec<MessageData>) -> Vec<MessageData> {
    match NOTES.get() {
        Some(notes) => interleave(
            history,
            notes.chat(agent),
            crate::record_loader::HISTORY_MESSAGE_LIMIT,
        ),
        None => history,
    }
}

#[cfg(test)]
mod tests;
