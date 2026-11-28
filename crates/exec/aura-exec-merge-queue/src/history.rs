//! A queue's full history: every entry that left the queue file, one JSON
//! line each, oldest first, in `merge-history.jsonl` next to it.
//!
//! ## Invariants
//!
//! - The file only grows by whole lines, under the queue's state lock
//!   ([`crate::store::QueueStore::update_archiving`]), and the queue file's
//!   [`HistoryMark`] says how much of it is committed. An append first
//!   cuts the file back to the mark, so a crash between writing the
//!   history and saving the queue file leaves no entry twice.
//! - Readers read only up to a mark they took from the queue file, so a
//!   half-written append is never read.
//! - A page costs the same however long the history is: it is read
//!   backwards from the mark, a block at a time.
//!
//! Everything here is blocking file IO; async callers run it on a
//! blocking thread.

use crate::entry::MergeEntry;
use crate::error::QueueError;
use crate::state::HistoryMark;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Name of the history file in the queue directory.
pub const HISTORY_FILE: &str = "merge-history.jsonl";
/// How much of the file a backwards read takes at a time.
const BLOCK: u64 = 64 * 1024;

/// A queue's history file.
#[derive(Debug, Clone)]
pub(crate) struct History {
    path: PathBuf,
}

impl History {
    /// The history kept in `dir`.
    pub(crate) fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(HISTORY_FILE),
        }
    }

    /// Append `entries` after what `mark` committed; returns the mark that
    /// commits them too. A file cut shorter than `mark` (by hand, or a
    /// disk that lost its tail) is counted again and appended to as it is.
    ///
    /// # Errors
    /// The file can't be opened, read or written.
    pub(crate) fn append(
        &self,
        mark: HistoryMark,
        entries: &[MergeEntry],
    ) -> Result<HistoryMark, QueueError> {
        let io = |e| QueueError::io(&self.path, e);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.path)
            .map_err(io)?;
        let len = file.metadata().map_err(io)?.len();
        let mut mark = mark;
        if len >= mark.bytes {
            // Past the mark is an append whose queue file was never saved.
            file.set_len(mark.bytes).map_err(io)?;
        } else {
            tracing::warn!(path = %self.path.display(), "The merge history is shorter than the queue says; counting it again");
            mark = count(&mut file, len).map_err(io)?;
            // A torn last line goes; whole lines stay.
            file.set_len(mark.bytes).map_err(io)?;
        }
        file.seek(SeekFrom::Start(mark.bytes)).map_err(io)?;
        let mut text = Vec::new();
        for entry in entries {
            serde_json::to_writer(&mut text, entry)
                .map_err(|e| QueueError::Corrupt(format!("encoding a history entry: {e}")))?;
            text.push(b'\n');
        }
        file.write_all(&text).map_err(io)?;
        file.sync_all().map_err(io)?;
        Ok(HistoryMark {
            count: mark.count + entries.len() as u64,
            bytes: mark.bytes + text.len() as u64,
        })
    }

    /// Up to `take` entries of what `mark` commits, newest first, past the
    /// newest `skip`. A line that doesn't read as an entry is skipped
    /// (logged).
    ///
    /// # Errors
    /// The file exists but can't be read.
    pub(crate) fn newest(
        &self,
        mark: HistoryMark,
        skip: u64,
        take: usize,
    ) -> Result<Vec<MergeEntry>, QueueError> {
        let io = |e| QueueError::io(&self.path, e);
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io(e)),
        };
        let end = mark.bytes.min(file.metadata().map_err(io)?.len());
        let mut lines = BackwardLines::new(end);
        let mut found = Vec::with_capacity(take);
        let mut skipped = 0;
        while found.len() < take {
            let Some(line) = lines.next(&mut file).map_err(io)? else {
                break;
            };
            if skipped < skip {
                skipped += 1;
                continue;
            }
            match serde_json::from_slice::<MergeEntry>(&line) {
                Ok(entry) => found.push(entry),
                Err(error) => {
                    tracing::warn!(path = %self.path.display(), %error, "Skipped an unreadable merge history line");
                }
            }
        }
        Ok(found)
    }
}

/// The whole lines of the first `len` bytes of `file`, as a mark.
fn count(file: &mut File, len: u64) -> std::io::Result<HistoryMark> {
    file.seek(SeekFrom::Start(0))?;
    let mut reader = BufReader::new(file.take(len));
    let mut mark = HistoryMark::default();
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)? as u64;
        if read == 0 || line.last() != Some(&b'\n') {
            return Ok(mark);
        }
        mark.count += 1;
        mark.bytes += read;
    }
}

/// The lines of a file before `end`, last first, read a block at a time.
struct BackwardLines {
    /// Where the bytes not yet read end
    pos: u64,
    /// Read bytes not yet split off, from `pos`
    buffer: Vec<u8>,
}

impl BackwardLines {
    fn new(end: u64) -> Self {
        Self {
            pos: end,
            buffer: Vec::new(),
        }
    }

    /// The line before the last one returned, without its newline;
    /// empty lines are passed over.
    fn next(&mut self, file: &mut File) -> std::io::Result<Option<Vec<u8>>> {
        loop {
            if let Some(at) = self.buffer.iter().rposition(|&b| b == b'\n') {
                let line = self.buffer.split_off(at + 1);
                self.buffer.pop();
                if !line.is_empty() {
                    return Ok(Some(line));
                }
                continue;
            }
            if self.pos == 0 {
                let line = std::mem::take(&mut self.buffer);
                return Ok((!line.is_empty()).then_some(line));
            }
            let start = self.pos.saturating_sub(BLOCK);
            let mut block = vec![0; usize::try_from(self.pos - start).unwrap_or(0)];
            file.seek(SeekFrom::Start(start))?;
            file.read_exact(&mut block)?;
            block.append(&mut self.buffer);
            self.buffer = block;
            self.pos = start;
        }
    }
}

#[cfg(test)]
mod tests;
