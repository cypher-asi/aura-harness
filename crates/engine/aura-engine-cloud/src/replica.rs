//! Mirroring a cloud node's record into the main RocksDB.
//!
//! The hub pulls `GET /agents/:id/record` pages from its local head + 1,
//! checks them with [`verify_page`], and hands them to a
//! [`ReplicaSink`], which appends them verbatim in one batch. The node
//! is the agent's single writer while the agent is cloud; the local
//! store only ever receives copies of what it committed.
//!
//! ## Invariants
//!
//! - Seqs continue the local head with no gap or duplicate.
//! - Every entry belongs to the agent and its transaction hash matches
//!   its payload (chained to the previous entry's hash or unchained, the
//!   two forms the kernel writes).
//! - A page is applied all or nothing; a page whose entries are all
//!   already present changes nothing.

use aura_core_types::{AgentId, Hash, RecordEntry};
use thiserror::Error;

use crate::error::CloudResult;

/// Why a page was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReplicaError {
    /// The page skips seqs.
    #[error("record page for {agent} starts at seq {got}, expected {expected}")]
    Gap {
        /// Agent, hex.
        agent: String,
        /// The seq it should start at.
        expected: u64,
        /// The seq it starts at.
        got: u64,
    },
    /// Seqs inside the page are not consecutive.
    #[error("record page for {agent} has seq {got} after {after}")]
    OutOfOrder {
        /// Agent, hex.
        agent: String,
        /// The seq before.
        after: u64,
        /// The seq found.
        got: u64,
    },
    /// An entry belongs to another agent.
    #[error("record page for {agent} holds an entry of {other} at seq {seq}")]
    WrongAgent {
        /// Agent, hex.
        agent: String,
        /// The other agent, hex.
        other: String,
        /// Its seq.
        seq: u64,
    },
    /// An entry's hash does not match its payload.
    #[error("record entry {seq} of {agent} has a hash that does not match its payload")]
    BadHash {
        /// Agent, hex.
        agent: String,
        /// Its seq.
        seq: u64,
    },
}

/// Where verified pages go.
pub trait ReplicaSink: Send + Sync {
    /// The local head of `agent` (0 = no entries).
    ///
    /// # Errors
    ///
    /// When the store can't be read.
    fn head(&self, agent: AgentId) -> CloudResult<u64>;

    /// The transaction hash of the entry at `seq`, if present.
    ///
    /// # Errors
    ///
    /// When the store can't be read.
    fn hash_at(&self, agent: AgentId, seq: u64) -> CloudResult<Option<Hash>>;

    /// Append verified `entries` in one atomic batch.
    ///
    /// # Errors
    ///
    /// When the store refuses the batch; nothing is written then.
    fn append_verified(&self, agent: AgentId, entries: &[RecordEntry]) -> CloudResult<()>;
}

/// Whether `entry`'s hash matches its payload, chained to `prev`, to the
/// transaction it references (a policy decision chains to its proposal,
/// which other entries may sit after), or to nothing.
#[must_use]
pub fn hash_holds(entry: &RecordEntry, prev: Option<&Hash>) -> bool {
    let tx = &entry.tx;
    // Records from before hashes were kept carry a zeroed hash.
    if tx.hash == Hash::default() {
        return true;
    }
    let chained_to = |link: &Hash| tx.hash == Hash::from_content_chained(&tx.payload, Some(link));
    tx.hash == Hash::from_content_chained(&tx.payload, None)
        || prev.is_some_and(chained_to)
        || tx.reference_tx_hash.as_ref().is_some_and(chained_to)
}

/// Check `entries` against the local head and the hash of the entry at
/// that head, and drop the ones already present. Returns what to append.
///
/// # Errors
///
/// [`ReplicaError`] naming the first problem.
pub fn verify_page(
    agent: AgentId,
    local_head: u64,
    head_hash: Option<Hash>,
    entries: &[RecordEntry],
) -> Result<&[RecordEntry], ReplicaError> {
    let hex = agent.to_hex();
    let skip = entries.iter().take_while(|e| e.seq <= local_head).count();
    let fresh = &entries[skip..];
    let Some(first) = fresh.first() else {
        return Ok(fresh);
    };
    if first.seq != local_head + 1 {
        return Err(ReplicaError::Gap {
            agent: hex,
            expected: local_head + 1,
            got: first.seq,
        });
    }
    let mut prev_seq = local_head;
    let mut prev_hash = head_hash;
    for entry in fresh {
        if entry.seq != prev_seq + 1 {
            return Err(ReplicaError::OutOfOrder {
                agent: hex,
                after: prev_seq,
                got: entry.seq,
            });
        }
        if entry.tx.agent_id != agent {
            return Err(ReplicaError::WrongAgent {
                agent: hex,
                other: entry.tx.agent_id.to_hex(),
                seq: entry.seq,
            });
        }
        if !hash_holds(entry, prev_hash.as_ref()) {
            return Err(ReplicaError::BadHash {
                agent: hex,
                seq: entry.seq,
            });
        }
        prev_seq = entry.seq;
        prev_hash = Some(entry.tx.hash);
    }
    Ok(fresh)
}

/// Verify and apply one page through `sink`. Returns how many entries
/// were appended.
///
/// # Errors
///
/// [`crate::CloudError::Node`] with the [`ReplicaError`] for a bad page;
/// the sink's error when the batch fails.
pub fn apply_page(
    sink: &dyn ReplicaSink,
    agent: AgentId,
    entries: &[RecordEntry],
) -> CloudResult<usize> {
    let head = sink.head(agent)?;
    let head_hash = if head == 0 {
        None
    } else {
        sink.hash_at(agent, head)?
    };
    let fresh =
        verify_page(agent, head, head_hash, entries).map_err(|err| crate::CloudError::Node {
            node: agent.to_hex(),
            op: "mirror record",
            message: err.to_string(),
        })?;
    if fresh.is_empty() {
        return Ok(0);
    }
    sink.append_verified(agent, fresh)?;
    Ok(fresh.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::{Transaction, TransactionType};
    use parking_lot::Mutex;
    use std::collections::BTreeMap;

    fn entries(
        agent: AgentId,
        seqs: std::ops::RangeInclusive<u64>,
        chained: bool,
    ) -> Vec<RecordEntry> {
        let mut prev: Option<Hash> = None;
        seqs.map(|seq| {
            let tx = Transaction::new_chained(
                agent,
                TransactionType::UserPrompt,
                format!("prompt {seq}").into_bytes(),
                if chained { prev.as_ref() } else { None },
            );
            prev = Some(tx.hash);
            RecordEntry::builder(seq, tx).build()
        })
        .collect()
    }

    #[derive(Default)]
    struct MemorySink(Mutex<BTreeMap<u64, RecordEntry>>);

    impl ReplicaSink for MemorySink {
        fn head(&self, _agent: AgentId) -> CloudResult<u64> {
            Ok(self.0.lock().keys().next_back().copied().unwrap_or(0))
        }
        fn hash_at(&self, _agent: AgentId, seq: u64) -> CloudResult<Option<Hash>> {
            Ok(self.0.lock().get(&seq).map(|e| e.tx.hash))
        }
        fn append_verified(&self, _agent: AgentId, entries: &[RecordEntry]) -> CloudResult<()> {
            let mut map = self.0.lock();
            for entry in entries {
                map.insert(entry.seq, entry.clone());
            }
            Ok(())
        }
    }

    #[test]
    fn pages_apply_in_order_and_reimports_change_nothing() {
        let agent = AgentId::new([1; 32]);
        let sink = MemorySink::default();
        let all = entries(agent, 1..=5, true);
        assert_eq!(apply_page(&sink, agent, &all[..3]).expect("first"), 3);
        assert_eq!(apply_page(&sink, agent, &all).expect("overlap"), 2);
        assert_eq!(apply_page(&sink, agent, &all).expect("again"), 0);
        assert_eq!(sink.head(agent).expect("head"), 5);
    }

    #[test]
    fn unchained_entries_verify_too() {
        let agent = AgentId::new([1; 32]);
        let page = entries(agent, 1..=3, false);
        assert_eq!(verify_page(agent, 0, None, &page).expect("ok").len(), 3);
    }

    #[test]
    fn gaps_are_refused() {
        let agent = AgentId::new([1; 32]);
        let page = entries(agent, 3..=4, true);
        assert!(matches!(
            verify_page(agent, 1, None, &page),
            Err(ReplicaError::Gap {
                expected: 2,
                got: 3,
                ..
            })
        ));
    }

    #[test]
    fn duplicates_inside_a_page_are_refused() {
        let agent = AgentId::new([1; 32]);
        let mut page = entries(agent, 1..=2, true);
        page.push(page[1].clone());
        assert!(matches!(
            verify_page(agent, 0, None, &page),
            Err(ReplicaError::OutOfOrder {
                after: 2,
                got: 2,
                ..
            })
        ));
    }

    #[test]
    fn a_decision_chained_to_an_earlier_proposal_verifies() {
        let agent = AgentId::new([1; 32]);
        let mut page = entries(agent, 1..=3, true);
        let proposal = page[0].tx.hash;
        let mut decision = Transaction::new_chained(
            agent,
            TransactionType::PolicyDecision,
            b"{\"verdict\":\"allow\"}".to_vec(),
            Some(&proposal),
        );
        decision.reference_tx_hash = Some(proposal);
        page.push(RecordEntry::builder(4, decision).build());
        assert_eq!(verify_page(agent, 0, None, &page).expect("ok").len(), 4);

        page[3].tx.reference_tx_hash = Some(page[2].tx.hash);
        assert!(
            matches!(
                verify_page(agent, 0, None, &page),
                Err(ReplicaError::BadHash { seq: 4, .. })
            ),
            "the hash still has to match what it names"
        );
    }

    #[test]
    fn a_tampered_payload_is_refused() {
        let agent = AgentId::new([1; 32]);
        let mut page = entries(agent, 1..=2, true);
        page[1].tx.payload = bytes::Bytes::from_static(b"changed");
        assert!(matches!(
            verify_page(agent, 0, None, &page),
            Err(ReplicaError::BadHash { seq: 2, .. })
        ));
    }

    #[test]
    fn another_agents_entries_are_refused_and_nothing_is_written() {
        let agent = AgentId::new([1; 32]);
        let sink = MemorySink::default();
        let mut page = entries(agent, 1..=2, true);
        page.extend(entries(AgentId::new([2; 32]), 3..=3, true));
        assert!(apply_page(&sink, agent, &page).is_err());
        assert_eq!(sink.head(agent).expect("head"), 0, "all or nothing");
    }
}
