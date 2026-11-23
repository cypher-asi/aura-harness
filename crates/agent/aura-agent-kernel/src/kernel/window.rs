//! The context hashes of an agent's newest record entries.
//!
//! An entry's context hash mixes in those of the entries before it, up to
//! `record_window_size` of them ([`crate::hash_tx_with_window`]). Reading
//! them from the store decodes each of those entries in full, payloads
//! and all, for 32 bytes apiece, on every append. So the kernel keeps the
//! hashes of the entries it reads and writes, and uses them in place of
//! the store while the newest is the store's head. An entry anyone else
//! wrote (a spawn's audit record, another kernel for the agent) moves the
//! head past them, and the window is read from the store again.

use aura_core_types::{AgentId, ContextHash, RecordEntry};
use aura_store_db::Store;
use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

/// Context hashes of consecutive entries of one agent, ending at the
/// newest the kernel saw.
#[derive(Debug)]
pub(crate) struct RecentHashes {
    /// Oldest first
    hashes: VecDeque<ContextHash>,
    /// The seq of the last of `hashes`, or 0 with none
    newest: u64,
    /// Most hashes kept: the window's size
    limit: usize,
}

impl RecentHashes {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            hashes: VecDeque::with_capacity(limit),
            newest: 0,
            limit,
        }
    }

    /// What `scan_record(agent, from_seq, limit)` reads of the context
    /// hashes, when the store's head is `head`: entries are numbered
    /// from 1 with none missing, so those from `from_seq` (or 1) up to
    /// the head, `limit` at most. `None` when not all of them are kept.
    pub(crate) fn window(
        &self,
        from_seq: u64,
        limit: usize,
        head: u64,
    ) -> Option<Vec<ContextHash>> {
        if self.newest != head {
            return None;
        }
        let first = from_seq.max(1);
        let count = u64::try_from(limit)
            .ok()?
            .min((head + 1).saturating_sub(first));
        if count == 0 {
            return Some(Vec::new());
        }
        let kept = u64::try_from(self.hashes.len()).ok()?;
        let oldest = self.newest + 1 - kept;
        if first < oldest {
            return None;
        }
        let start = usize::try_from(first - oldest).ok()?;
        let end = start + usize::try_from(count).ok()?;
        Some(self.hashes.range(start..end).copied().collect())
    }

    /// Keep the hashes of `entries`, read from the store whose head is
    /// `head`, when they run without a gap to the head.
    fn refill(&mut self, entries: &[RecordEntry], head: u64) {
        let runs = entries
            .windows(2)
            .all(|pair| pair[1].seq == pair[0].seq + 1);
        if !runs || entries.last().map(|entry| entry.seq) != Some(head) {
            return;
        }
        self.hashes.clear();
        let skip = entries.len().saturating_sub(self.limit);
        self.hashes
            .extend(entries[skip..].iter().map(|entry| entry.context_hash));
        self.newest = head;
    }

    /// Keep the hash of `entry`, just appended.
    pub(crate) fn push(&mut self, entry: &RecordEntry) {
        if entry.seq <= self.newest {
            return;
        }
        if entry.seq != self.newest + 1 {
            // Entries in between were written by someone else.
            self.hashes.clear();
        }
        self.hashes.push_back(entry.context_hash);
        self.newest = entry.seq;
        while self.hashes.len() > self.limit {
            self.hashes.pop_front();
        }
    }
}

/// The context hashes the entry at `next_seq` mixes in, oldest first:
/// those of the last `size` entries up to it.
pub(crate) fn window_hashes(
    store: &dyn Store,
    agent_id: AgentId,
    recent: &Mutex<RecentHashes>,
    next_seq: u64,
    size: usize,
) -> Result<Vec<ContextHash>, crate::KernelError> {
    let from_seq = next_seq.saturating_sub(size as u64);
    let head = store
        .get_head_seq(agent_id)
        .map_err(|e| crate::KernelError::Store(format!("get_head_seq: {e}")))?;
    let kept = recent
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .window(from_seq, size, head);
    if let Some(hashes) = kept {
        return Ok(hashes);
    }
    let entries = store
        .scan_record(agent_id, from_seq, size)
        .map_err(|e| crate::KernelError::Store(format!("scan_record: {e}")))?;
    recent
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .refill(&entries, head);
    Ok(entries.iter().map(|entry| entry.context_hash).collect())
}

/// Keep the hash of `entry`, just appended, in `recent`.
pub(crate) fn remember(recent: &Mutex<RecentHashes>, entry: &RecordEntry) {
    recent
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(entry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::{Hash, Transaction, TransactionType};
    use bytes::Bytes;

    fn entry(seq: u64) -> RecordEntry {
        let tx = Transaction::new(
            Hash::from_content(&seq.to_be_bytes()),
            AgentId::new([7; 32]),
            1000,
            TransactionType::UserPrompt,
            Bytes::from_static(b"payload"),
        );
        let mut hash = [0u8; 32];
        hash[..8].copy_from_slice(&seq.to_be_bytes());
        RecordEntry::builder(seq, tx).context_hash(hash).build()
    }

    /// What the store's scan returns of `head` entries numbered from 1.
    fn scanned(head: u64, from_seq: u64, limit: usize) -> Vec<ContextHash> {
        (from_seq.max(1)..=head)
            .take(limit)
            .map(|seq| entry(seq).context_hash)
            .collect()
    }

    #[test]
    fn the_hashes_kept_are_what_the_store_would_read() {
        let mut recent = RecentHashes::new(5);
        assert_eq!(recent.window(0, 5, 0), Some(Vec::new()), "no entries yet");
        for head in 1..=12 {
            recent.push(&entry(head));
            for next in head.saturating_sub(2)..=head + 3 {
                let from_seq = next.saturating_sub(5);
                let kept = recent.window(from_seq, 5, head);
                if let Some(kept) = kept {
                    assert_eq!(kept, scanned(head, from_seq, 5), "head {head}, next {next}");
                } else {
                    assert!(from_seq.max(1) + 5 <= head, "only older ones are missing");
                }
            }
        }
        assert_eq!(recent.hashes.len(), 5, "no more than the window");
    }

    #[test]
    fn an_entry_written_by_anyone_else_means_reading_the_store() {
        let mut recent = RecentHashes::new(5);
        for seq in 1..=6 {
            recent.push(&entry(seq));
        }
        assert_eq!(recent.window(2, 5, 7), None, "the head moved past them");

        let read: Vec<RecordEntry> = (3..=7).map(entry).collect();
        recent.refill(&read, 7);
        assert_eq!(recent.window(3, 5, 7), Some(scanned(7, 3, 5)));

        recent.push(&entry(9));
        assert_eq!(recent.window(4, 5, 9), None, "8 is missing");
        assert_eq!(recent.window(9, 5, 9), Some(scanned(9, 9, 5)));
    }

    #[test]
    fn a_read_that_stops_short_of_the_head_is_not_kept() {
        let mut recent = RecentHashes::new(5);
        let read: Vec<RecordEntry> = (1..=5).map(entry).collect();
        recent.refill(&read, 9);
        assert_eq!(recent.window(5, 5, 9), None);
        let gapped: Vec<RecordEntry> = [1, 2, 4].into_iter().map(entry).collect();
        recent.refill(&gapped, 4);
        assert_eq!(recent.window(0, 5, 4), None);
    }
}
