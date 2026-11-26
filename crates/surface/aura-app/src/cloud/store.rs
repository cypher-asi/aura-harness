//! Store-backed halves of the cloud seams: the machine ledger writer and
//! the record mirror's sink.
//!
//! Both are declared exceptions to "only an agent's kernel appends to
//! its record" (`docs/invariants.md`, *Declared Exceptions*):
//!
//! - [`StoreLedger`] is the only writer of the ledger agents
//!   (`0://cloud/vm/...`, `0://cloud/project/...`), which no kernel runs.
//! - [`StoreSink`] copies entries a cloud node's kernel committed, while
//!   that node is the agent's single writer.

use std::collections::HashMap;
use std::sync::Arc;

use aura_core_types::{
    AgentId, Hash, PlacementPayload, RecordEntry, SystemKind, Transaction, TransactionType,
    VmPayload,
};
use aura_engine_cloud::replica::ReplicaSink;
use aura_engine_cloud::{CloudError, CloudResult};
use aura_store_db::Store;
use parking_lot::Mutex;

/// The agent id of a ZNS id, as the registry derives it.
#[must_use]
pub fn agent_id_of(zns: &str) -> AgentId {
    AgentId::new(*blake3::hash(zns.as_bytes()).as_bytes())
}

/// Appends `System` rows to agents no kernel runs, one writer per agent.
pub struct StoreLedger {
    store: Arc<dyn Store>,
    locks: Mutex<HashMap<AgentId, Arc<Mutex<()>>>>,
}

impl StoreLedger {
    /// A ledger writer over `store`.
    #[must_use]
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self {
            store,
            locks: Mutex::new(HashMap::new()),
        }
    }

    /// Append one `System` row with `payload` to `agent`.
    ///
    /// # Errors
    ///
    /// When the store refuses the append.
    pub fn append_system(&self, agent: AgentId, payload: Vec<u8>) -> CloudResult<u64> {
        let lock = Arc::clone(self.locks.lock().entry(agent).or_default());
        let _guard = lock.lock();
        let head = self.store.get_head_seq(agent).map_err(store_error)?;
        let prev = if head == 0 {
            None
        } else {
            Some(
                self.store
                    .get_record_entry(agent, head)
                    .map_err(store_error)?
                    .tx
                    .hash,
            )
        };
        let tx = Transaction::new_chained(agent, TransactionType::System, payload, prev.as_ref());
        let entry = RecordEntry::builder(head + 1, tx).build();
        self.store
            .append_entry_direct(agent, head + 1, &entry)
            .map_err(store_error)?;
        Ok(head + 1)
    }

    /// Every [`VmPayload`] row of the ledger agent `zns`, oldest first,
    /// at most the last `limit`.
    #[must_use]
    pub fn rows(&self, zns: &str, limit: usize) -> Vec<VmPayload> {
        let agent = agent_id_of(zns);
        let head = self.store.get_head_seq(agent).unwrap_or(0);
        let from = head.saturating_sub(limit as u64) + 1;
        self.store
            .scan_record(agent, from, limit)
            .unwrap_or_default()
            .iter()
            .filter(|entry| entry.tx.tx_type == TransactionType::System)
            .filter_map(|entry| VmPayload::from_bytes(&entry.tx.payload))
            .collect()
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "used as a `map_err` callback, which passes the error by value"
)]
fn store_error(err: aura_store_db::StoreError) -> CloudError {
    CloudError::io("cloud ledger", std::io::Error::other(err.to_string()))
}

impl aura_engine_cloud::fleet::VmLedger for StoreLedger {
    fn record(&self, ledger_zns: &str, payload: &VmPayload) {
        if let Err(err) = self.append_system(agent_id_of(ledger_zns), payload.to_bytes()) {
            tracing::warn!(ledger = %ledger_zns, event = payload.event.as_str(), error = %err, "could not record a machine row");
        }
    }
}

/// Record the moment an agent's single writer moved, on the agent's
/// machine ledger (`ledger_zns`). Never on the agent's own record: its
/// next seq belongs to whichever kernel writes for it after the move.
///
/// # Errors
///
/// When the store refuses the append.
pub fn record_placement(
    ledger: &StoreLedger,
    ledger_zns: &str,
    payload: &PlacementPayload,
) -> CloudResult<u64> {
    debug_assert_eq!(payload.system_kind, SystemKind::Placement);
    ledger.append_system(agent_id_of(ledger_zns), payload.to_bytes())
}

/// Mirror sink: verified pages go into the local store verbatim.
pub struct StoreSink(pub Arc<dyn Store>);

impl ReplicaSink for StoreSink {
    fn head(&self, agent: AgentId) -> CloudResult<u64> {
        self.0.get_head_seq(agent).map_err(store_error)
    }

    fn hash_at(&self, agent: AgentId, seq: u64) -> CloudResult<Option<Hash>> {
        match self.0.get_record_entry(agent, seq) {
            Ok(entry) => Ok(Some(entry.tx.hash)),
            Err(_) => Ok(None),
        }
    }

    fn append_verified(&self, agent: AgentId, entries: &[RecordEntry]) -> CloudResult<()> {
        let Some(first) = entries.first() else {
            return Ok(());
        };
        self.0
            .append_replica_batch(agent, first.seq, entries)
            .map_err(store_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::{Placement, VmEvent};
    use aura_engine_cloud::fleet::VmLedger;
    use aura_engine_cloud::replica::apply_page;
    use aura_store_db::RocksStore;

    fn store() -> (Arc<dyn Store>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store: Arc<dyn Store> = Arc::new(RocksStore::open(dir.path(), false).expect("open"));
        (store, dir)
    }

    #[test]
    fn ledger_rows_append_and_read_back_in_order() {
        let (store, _dir) = store();
        let ledger = StoreLedger::new(Arc::clone(&store));
        let zns = "0://cloud/vm/terminal/re36";
        ledger.record(zns, &VmPayload::new(VmEvent::Created, "id", "m"));
        ledger.record(zns, &VmPayload::new(VmEvent::Hibernated, "id", "m"));
        let rows = ledger.rows(zns, 10);
        assert_eq!(
            rows.iter().map(|r| r.event).collect::<Vec<_>>(),
            vec![VmEvent::Created, VmEvent::Hibernated]
        );
        assert_eq!(store.get_head_seq(agent_id_of(zns)).expect("head"), 2);
        // Rows chain: the second's hash covers the first's.
        let second = store.get_record_entry(agent_id_of(zns), 2).expect("entry");
        let first = store.get_record_entry(agent_id_of(zns), 1).expect("entry");
        assert!(aura_engine_cloud::replica::hash_holds(
            &second,
            Some(&first.tx.hash)
        ));
    }

    #[test]
    fn placement_rows_land_on_the_machine_ledger_not_the_agent() {
        let (store, _dir) = store();
        let ledger = StoreLedger::new(Arc::clone(&store));
        let agent = agent_id_of("0://terminal/re36");
        let zns = "0://cloud/vm/terminal/re36";
        record_placement(
            &ledger,
            zns,
            &PlacementPayload::new(Placement::Local, Placement::Cloud, Some("m".into())),
        )
        .expect("recorded");
        assert_eq!(
            store.get_head_seq(agent).expect("head"),
            0,
            "the agent's seqs stay its writer's"
        );
        let entry = store.get_record_entry(agent_id_of(zns), 1).expect("entry");
        assert_eq!(
            PlacementPayload::from_bytes(&entry.tx.payload).map(|p| p.to),
            Some(Placement::Cloud)
        );
    }

    #[test]
    fn the_sink_mirrors_a_remote_record_verbatim() {
        let (store, _dir) = store();
        let sink = StoreSink(Arc::clone(&store));
        let agent = AgentId::new([3; 32]);
        let mut prev: Option<Hash> = None;
        let page: Vec<RecordEntry> = (1..=3)
            .map(|seq| {
                let tx = Transaction::new_chained(
                    agent,
                    TransactionType::UserPrompt,
                    format!("p{seq}").into_bytes(),
                    prev.as_ref(),
                );
                prev = Some(tx.hash);
                RecordEntry::builder(seq, tx).build()
            })
            .collect();
        assert_eq!(apply_page(&sink, agent, &page[..2]).expect("first"), 2);
        assert_eq!(apply_page(&sink, agent, &page).expect("rest"), 1);
        assert_eq!(store.get_record_entry(agent, 3).expect("entry"), page[2]);
    }
}
