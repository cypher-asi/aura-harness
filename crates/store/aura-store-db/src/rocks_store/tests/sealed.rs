//! Sealed state-at-rest (Swarm TEE upgrade phase 5).

use super::support::*;
use crate::cf;
use crate::error::StoreError;
use crate::rocks_store::*;
use crate::seal::SealCipher;
use crate::store::ReadStore;
use crate::store::WriteStore;
use aura_core_types::{AgentId, RecordEntry, UserToolDefaults};
use std::sync::Arc;
use tempfile::TempDir;

#[test]
fn sealed_store_record_roundtrip_and_ciphertext_on_disk() {
    let (store, _dir) = create_sealed_test_store([42u8; 32]);
    let agent_id = AgentId::generate();
    let tx = create_test_tx(agent_id);
    let entry = RecordEntry::builder(1, tx).build();

    store.append_entry_direct(agent_id, 1, &entry).unwrap();

    // Read path decrypts transparently.
    let retrieved = store.get_record_entry(agent_id, 1).unwrap();
    assert_eq!(retrieved.seq, 1);
    assert_eq!(store.scan_record(agent_id, 1, 10).unwrap().len(), 1);
    assert_eq!(
        store.scan_record_descending(agent_id, 1, 10).unwrap().len(),
        1
    );

    // On disk the value is a sealed envelope, not JSON.
    let raw = raw_record_bytes(&store, agent_id, 1);
    assert!(
        SealCipher::is_sealed(&raw),
        "on-disk value must carry the seal magic"
    );
    assert!(serde_json::from_slice::<RecordEntry>(&raw).is_err());
    assert!(
        !raw.windows(b"test payload".len())
            .any(|w| w == b"test payload"),
        "plaintext payload must not appear in the sealed on-disk value"
    );
}

#[test]
fn sealed_store_inbox_roundtrip() {
    let (store, _dir) = create_sealed_test_store([42u8; 32]);
    let agent_id = AgentId::generate();
    let tx = create_test_tx(agent_id);

    store.enqueue_tx(&tx).unwrap();
    let (token, dequeued) = store.dequeue_tx(agent_id).unwrap().unwrap();
    assert_eq!(dequeued.hash, tx.hash);

    let entry = RecordEntry::builder(1, tx).build();
    store
        .append_entry_atomic(agent_id, 1, &entry, token.inbox_seq())
        .unwrap();
    assert!(!store.has_pending_tx(agent_id).unwrap());
}

#[test]
fn sealed_store_user_tool_defaults_roundtrip() {
    let (store, _dir) = create_sealed_test_store([42u8; 32]);
    let defaults = UserToolDefaults::default();

    store.put_user_tool_defaults("user-1", &defaults).unwrap();
    let loaded = store.get_user_tool_defaults("user-1").unwrap();
    assert!(loaded.is_some());

    // Raw bytes on disk are sealed.
    let cf = store.db_handle().cf_handle(cf::USER_TOOL_DEFAULTS).unwrap();
    let raw = store.db_handle().get_cf(&cf, b"user-1").unwrap().unwrap();
    assert!(SealCipher::is_sealed(&raw));
}

/// Plaintext mode must remain byte-for-byte identical to the historical
/// on-disk format — legacy agents are untouched by the sealed-mode code.
#[test]
fn plaintext_store_on_disk_format_is_unchanged() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();
    let tx = create_test_tx(agent_id);
    let entry = RecordEntry::builder(1, tx).build();

    store.append_entry_direct(agent_id, 1, &entry).unwrap();

    let raw = raw_record_bytes(&store, agent_id, 1);
    assert_eq!(
        raw,
        serde_json::to_vec(&entry).unwrap(),
        "plaintext mode must write the exact legacy JSON bytes"
    );
    assert!(!SealCipher::is_sealed(&raw));
}

/// Opening sealed data with the wrong DEK must fail loudly, never return
/// garbage or silently fall back to plaintext.
#[test]
fn sealed_store_wrong_key_fails_reads() {
    let dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let entry = RecordEntry::builder(1, create_test_tx(agent_id)).build();

    {
        let cipher = Arc::new(SealCipher::new(&[1u8; 32]));
        let store = RocksStore::open_sealed(dir.path(), false, Some(cipher)).unwrap();
        store.append_entry_direct(agent_id, 1, &entry).unwrap();
    }

    let wrong = Arc::new(SealCipher::new(&[2u8; 32]));
    let store = RocksStore::open_sealed(dir.path(), false, Some(wrong)).unwrap();
    let err = store.get_record_entry(agent_id, 1).unwrap_err();
    assert!(matches!(err, StoreError::Deserialization(_)));
}
