//! Fixtures shared by the `RocksStore` unit tests.

use crate::cf;
use crate::keys::{KeyCodec, RecordKey};
use crate::rocks_store::*;
use crate::seal::SealCipher;
use aura_core_types::{AgentId, Transaction};
use aura_core_types::{Hash, TransactionType};
use bytes::Bytes;
use std::sync::Arc;
use tempfile::TempDir;

pub(super) fn create_test_store() -> (RocksStore, TempDir) {
    let dir = TempDir::new().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    (store, dir)
}

pub(super) fn create_test_tx(agent_id: AgentId) -> Transaction {
    Transaction::new(
        Hash::from_content(b"test"),
        agent_id,
        1000,
        TransactionType::UserPrompt,
        Bytes::from_static(b"test payload"),
    )
}

pub(super) fn create_sealed_test_store(key: [u8; 32]) -> (RocksStore, TempDir) {
    let dir = TempDir::new().unwrap();
    let cipher = Arc::new(SealCipher::new(&key));
    let store = RocksStore::open_sealed(dir.path(), false, Some(cipher)).unwrap();
    (store, dir)
}

/// Read the raw on-disk bytes for an agent's record entry at `seq`,
/// bypassing the store's open/seal layer.
pub(super) fn raw_record_bytes(store: &RocksStore, agent_id: AgentId, seq: u64) -> Vec<u8> {
    let cf = store.db_handle().cf_handle(cf::RECORD).unwrap();
    store
        .db_handle()
        .get_cf(&cf, RecordKey::new(agent_id, seq).encode())
        .unwrap()
        .expect("record entry must exist on disk")
}
