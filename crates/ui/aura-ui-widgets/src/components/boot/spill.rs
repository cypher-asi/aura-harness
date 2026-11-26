//! The spill file: log lines a console dropped from memory, kept on disk.

use aura_ui::events::BootLogLine;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Numbers spill files, so two consoles never share one.
static NEXT_SPILL: AtomicU64 = AtomicU64::new(0);

/// The log lines a console dropped from memory, in a temporary file that
/// goes when the last copy of the console does.
#[derive(Debug)]
pub(super) struct Spill {
    pub(super) path: PathBuf,
    file: File,
    /// Where each line starts in the file
    pub(super) starts: Vec<u64>,
    /// Where the file ends
    end: u64,
}

impl Spill {
    pub(super) fn create() -> Option<Self> {
        let path = std::env::temp_dir().join(format!(
            "aura-boot-{}-{}.log",
            std::process::id(),
            NEXT_SPILL.fetch_add(1, Ordering::Relaxed)
        ));
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .ok()?;
        Some(Self {
            path,
            file,
            starts: Vec::new(),
            end: 0,
        })
    }

    /// Add `lines` at the end. Each is a byte saying whether it was
    /// stderr, then its text.
    pub(super) fn append(&mut self, lines: &[BootLogLine]) -> std::io::Result<()> {
        let mut bytes = Vec::new();
        let mut starts = Vec::with_capacity(lines.len());
        for line in lines {
            starts.push(self.end + bytes.len() as u64);
            bytes.push(if line.stderr { b'2' } else { b'1' });
            bytes.extend_from_slice(line.text.as_bytes());
        }
        self.file.seek(SeekFrom::Start(self.end))?;
        self.file.write_all(&bytes)?;
        self.starts.extend(starts);
        self.end += bytes.len() as u64;
        Ok(())
    }

    /// Lines `from..to` of the file.
    pub(super) fn read(&mut self, from: usize, to: usize) -> std::io::Result<Vec<BootLogLine>> {
        let start = self.starts[from];
        let stop = self.starts.get(to).copied().unwrap_or(self.end);
        let mut bytes = vec![0; usize::try_from(stop - start).unwrap_or(0)];
        self.file.seek(SeekFrom::Start(start))?;
        self.file.read_exact(&mut bytes)?;
        Ok((from..to)
            .map(|i| {
                let at = usize::try_from(self.starts[i] - start).unwrap_or(0);
                let next = self
                    .starts
                    .get(i + 1)
                    .map_or(bytes.len(), |s| usize::try_from(s - start).unwrap_or(0))
                    .min(bytes.len());
                let line = &bytes[at.min(next)..next];
                BootLogLine {
                    stderr: line.first() == Some(&b'2'),
                    text: String::from_utf8_lossy(line.get(1..).unwrap_or_default()).into_owned(),
                }
            })
            .collect())
    }
}

impl Drop for Spill {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A console's spill file, shared by its copies.
#[derive(Debug, Clone)]
pub(super) struct SpillFile(pub(super) Arc<Mutex<Spill>>);

impl SpillFile {
    pub(super) fn lock(&self) -> MutexGuard<'_, Spill> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl PartialEq for SpillFile {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for SpillFile {}
