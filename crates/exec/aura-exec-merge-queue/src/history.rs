//! Every finished merge, kept for good: the queue file keeps only the
//! newest ones ([`crate::Limits::keep_finished`]), the history keeps all.
//!
//! ## Format
//!
//! [`HISTORY_FILE`], next to the queue file, holds one finished
//! [`MergeEntry`] per line as JSON, in the order they finished (JSON
//! Lines). It is only ever appended to, so a line never moves and its
//! byte offset is a stable cursor: a page of older merges is "the
//! entries before offset N" ([`read_page`]), read backwards from there in
//! fixed-size chunks, so the newest page costs the same however long the
//! history grows.
//!
//! ## Invariants
//!
//! - Entries are appended under the queue's state lock, before the queue
//!   file that finished them is saved ([`crate::store::QueueStore::update`]).
//!   A crash in between can leave an entry in the history twice (it
//!   finishes again after recovery); readers keep the newest line of an
//!   id, so it shows once.
//! - A torn last line (a crash mid-append) or any line that isn't an
//!   entry is skipped, never fatal; the next append starts on a line of
//!   its own.
//! - A history made from an older queue file, with no history yet, is
//!   seeded from the finished entries that file kept ([`seed`]).
//!
//! Everything here is blocking file IO; async callers run it on a
//! blocking thread.

use crate::entry::{EntryId, MergeEntry};
use crate::error::QueueError;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Name of the history file in the queue directory.
pub const HISTORY_FILE: &str = "merge-history.jsonl";

/// Finished merges a page holds.
pub const PAGE: usize = 100;

/// Bytes read at a time when walking the history backwards.
const CHUNK: u64 = 64 * 1024;

/// Newest finished merges before a cursor, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryPage {
    /// The merges, newest finished first, each id once
    pub entries: Vec<MergeEntry>,
    /// Cursor of the next older page; `None` when this one reaches the
    /// start of the history
    pub older: Option<u64>,
}

/// The history file of the queue in `dir`.
#[must_use]
pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(HISTORY_FILE)
}

/// Append `entries`, finished, to the history in `dir`, synced.
///
/// # Errors
/// The directory or file can't be written.
pub fn append(dir: &Path, entries: &[MergeEntry]) -> Result<(), QueueError> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut text = String::new();
    for entry in entries {
        let line = serde_json::to_string(entry)
            .map_err(|e| QueueError::Corrupt(format!("encoding a finished merge: {e}")))?;
        text.push_str(&line);
        text.push('\n');
    }
    fs::create_dir_all(dir).map_err(|e| QueueError::io(dir, e))?;
    let path = path_in(dir);
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&path)
        .map_err(|e| QueueError::io(&path, e))?;
    // A torn last line from a crash would swallow the first new one.
    if ends_mid_line(&mut file).map_err(|e| QueueError::io(&path, e))? {
        text.insert(0, '\n');
    }
    file.write_all(text.as_bytes())
        .map_err(|e| QueueError::io(&path, e))?;
    file.sync_all().map_err(|e| QueueError::io(&path, e))
}

/// Start the history in `dir` from `finished`, the finished entries an
/// older queue file kept, oldest finished first, when there is no
/// history yet. Does nothing once one exists.
///
/// # Errors
/// The file can't be written.
pub fn seed(dir: &Path, finished: &[MergeEntry]) -> Result<(), QueueError> {
    if path_in(dir).exists() || finished.is_empty() {
        return Ok(());
    }
    let mut ordered = finished.to_vec();
    ordered.sort_by_key(|entry| entry.finished_at.unwrap_or(entry.enqueued_at));
    append(dir, &ordered)
}

/// Up to `limit` of the newest finished merges in `dir`'s history that
/// sit before byte `before` (the whole file with `None`), newest first.
/// No history is an empty page.
///
/// # Errors
/// The file exists but can't be read.
pub fn read_page(dir: &Path, before: Option<u64>, limit: usize) -> Result<HistoryPage, QueueError> {
    let path = path_in(dir);
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HistoryPage::default()),
        Err(e) => return Err(QueueError::io(&path, e)),
    };
    let len = file.metadata().map_err(|e| QueueError::io(&path, e))?.len();
    let end = before.map_or(len, |before| before.min(len));
    walk_back(&mut file, end, limit).map_err(|e| QueueError::io(&path, e))
}

/// Read lines backwards from byte `end` of `file` until `limit` entries
/// are found or the start is reached.
fn walk_back(file: &mut File, end: u64, limit: usize) -> std::io::Result<HistoryPage> {
    let mut page = Lines::default();
    // Bytes after the last newline read so far: the end of a line that
    // starts in an earlier chunk.
    let mut tail: Vec<u8> = Vec::new();
    let mut at = end;
    while at > 0 && page.entries.len() < limit {
        let start = at.saturating_sub(CHUNK);
        let mut chunk = vec![0; usize::try_from(at - start).unwrap_or(0)];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&tail);
        let mut cut = chunk.len();
        while let Some(newline) = chunk[..cut].iter().rposition(|&b| b == b'\n') {
            page.take(&chunk[newline + 1..cut], offset(start, newline + 1));
            cut = newline;
            if page.entries.len() >= limit {
                return Ok(page.finish(Some(offset(start, cut + 1))));
            }
        }
        tail = chunk[..cut].to_vec();
        at = start;
    }
    if page.entries.len() < limit {
        page.take(&tail, 0);
    }
    let older = page.oldest_offset.filter(|&offset| offset > 0);
    Ok(page.finish(older))
}

/// Byte `i` of a chunk read from byte `start` of the file.
fn offset(start: u64, i: usize) -> u64 {
    start.saturating_add(u64::try_from(i).unwrap_or(u64::MAX))
}

/// Entries collected walking backwards.
#[derive(Default)]
struct Lines {
    entries: Vec<MergeEntry>,
    seen: HashSet<EntryId>,
    /// Byte offset of the oldest line read
    oldest_offset: Option<u64>,
}

impl Lines {
    /// Keep the entry on `line`, which starts at byte `offset`, unless it
    /// is empty, not an entry, or a newer line had its id.
    fn take(&mut self, line: &[u8], offset: u64) {
        self.oldest_offset = Some(offset);
        let line = line.trim_ascii();
        if line.is_empty() {
            return;
        }
        match serde_json::from_slice::<MergeEntry>(line) {
            Ok(entry) => {
                if self.seen.insert(entry.id.clone()) {
                    self.entries.push(entry);
                }
            }
            Err(error) => tracing::warn!(%error, offset, "Skipping a merge history line"),
        }
    }

    fn finish(self, older: Option<u64>) -> HistoryPage {
        HistoryPage {
            entries: self.entries,
            older,
        }
    }
}

/// Whether `file` ends without a newline, after a torn append.
fn ends_mid_line(file: &mut File) -> std::io::Result<bool> {
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(false);
    }
    let mut last = [0u8; 1];
    file.seek(SeekFrom::Start(len - 1))?;
    file.read_exact(&mut last)?;
    Ok(last[0] != b'\n')
}

#[cfg(test)]
mod tests;
