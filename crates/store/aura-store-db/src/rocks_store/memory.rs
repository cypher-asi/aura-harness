//! What `RocksDB` itself holds in memory, outside the Rust heap, as its
//! own properties report it.

use super::open::EXPECTED_COLUMN_FAMILIES;
use super::RocksStore;

/// Native memory of a store, in bytes, as `RocksDB` reports it. A
/// property it can't read counts as 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NativeMemory {
    /// The block cache in use (`rocksdb.block-cache-usage`). One cache is
    /// shared by every store the process opens, so this is the process's.
    pub block_cache: u64,
    /// Of that, what is pinned (`rocksdb.block-cache-pinned-usage`)
    pub block_cache_pinned: u64,
    /// Memtables of every column family (`rocksdb.cur-size-all-mem-tables`)
    pub memtables: u64,
    /// Index and filter blocks held outside the block cache, every column
    /// family (`rocksdb.estimate-table-readers-mem`)
    pub table_readers: u64,
}

impl NativeMemory {
    /// Every part but the pinned share, which the block cache counts.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.block_cache + self.memtables + self.table_readers
    }
}

impl RocksStore {
    /// What `RocksDB` holds in memory for this store now.
    #[must_use]
    pub fn native_memory(&self) -> NativeMemory {
        let db = self.db_handle();
        let read = |name: &str| db.property_int_value(name).ok().flatten().unwrap_or(0);
        let per_family = |property: &str| -> u64 {
            EXPECTED_COLUMN_FAMILIES
                .iter()
                .filter_map(|family| db.cf_handle(family))
                .filter_map(|cf| db.property_int_value_cf(&cf, property).ok().flatten())
                .sum()
        };
        NativeMemory {
            block_cache: read("rocksdb.block-cache-usage"),
            block_cache_pinned: read("rocksdb.block-cache-pinned-usage"),
            memtables: per_family("rocksdb.cur-size-all-mem-tables"),
            table_readers: per_family("rocksdb.estimate-table-readers-mem"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::WriteStore;
    use aura_core_types::{AgentId, Hash, RecordEntry, Transaction, TransactionType};

    #[test]
    fn writes_show_in_the_memtables() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = RocksStore::open(dir.path(), false).unwrap();
        let before = store.native_memory();
        let agent = AgentId::generate();
        for seq in 1..=200u64 {
            let tx = Transaction::new(
                Hash::from_content(&seq.to_le_bytes()),
                agent,
                1000,
                TransactionType::UserPrompt,
                bytes::Bytes::from(vec![b'x'; 1024]),
            );
            let entry = RecordEntry::builder(seq, tx).build();
            store.append_entry_direct(agent, seq, &entry).unwrap();
        }
        let after = store.native_memory();
        assert!(after.memtables > before.memtables, "{before:?} {after:?}");
        assert!(after.total() >= after.memtables);
    }
}
