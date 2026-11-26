//! Reading and scanning the record log.

use super::support::*;
use crate::cf;
use crate::error::StoreError;
use crate::keys::{KeyCodec, RecordKey};
use crate::store::ReadStore;
use crate::store::WriteStore;
use aura_core_types::{AgentId, RecordEntry, Transaction};
use aura_core_types::{Decision, Hash, ProposalSet, TransactionType};
use bytes::Bytes;

#[test]
fn test_scan_record() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    for i in 1..=5 {
        let tx = create_test_tx(agent_id);
        store.enqueue_tx(&tx).unwrap();
        let (token, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();

        #[allow(clippy::cast_possible_truncation)] // i is always 1-5 in test
        let entry = RecordEntry::builder(i, tx)
            .context_hash([i as u8; 32])
            .build();

        store
            .append_entry_atomic(agent_id, i, &entry, token.inbox_seq())
            .unwrap();
    }

    let entries = store.scan_record(agent_id, 1, 10).unwrap();
    assert_eq!(entries.len(), 5);

    let entries = store.scan_record(agent_id, 3, 10).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].seq, 3);

    let entries = store.scan_record(agent_id, 1, 2).unwrap();
    assert_eq!(entries.len(), 2);

    for (from, limit) in [(1, 10), (3, 10), (2, 2), (9, 4)] {
        let full: Vec<_> = store
            .scan_record(agent_id, from, limit)
            .unwrap()
            .into_iter()
            .map(|entry| (entry.seq, entry.context_hash))
            .collect();
        let hashes = store.scan_context_hashes(agent_id, from, limit).unwrap();
        assert_eq!(hashes, full, "from {from}, limit {limit}");
    }
}

#[test]
fn test_scan_record_descending() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    for i in 1..=5 {
        let tx = create_test_tx(agent_id);
        store.enqueue_tx(&tx).unwrap();
        let (token, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();

        let entry = RecordEntry::builder(i, tx)
            .context_hash([i as u8; 32])
            .build();

        store
            .append_entry_atomic(agent_id, i, &entry, token.inbox_seq())
            .unwrap();
    }

    let entries = store.scan_record_descending(agent_id, 5, 3).unwrap();
    let seqs: Vec<_> = entries.iter().map(|entry| entry.seq).collect();
    assert_eq!(seqs, vec![5, 4, 3]);

    let entries = store.scan_record_descending(agent_id, 3, 10).unwrap();
    let seqs: Vec<_> = entries.iter().map(|entry| entry.seq).collect();
    assert_eq!(seqs, vec![3, 2, 1]);
}

#[test]
fn test_scan_empty_record() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let entries = store.scan_record(agent_id, 1, 10).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn test_scan_partial_range() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    for i in 1..=10 {
        let tx = create_test_tx(agent_id);
        store.enqueue_tx(&tx).unwrap();
        let (token, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
        let entry = RecordEntry::builder(i, tx).build();
        store
            .append_entry_atomic(agent_id, i, &entry, token.inbox_seq())
            .unwrap();
    }

    let entries = store.scan_record(agent_id, 5, 3).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].seq, 5);
    assert_eq!(entries[1].seq, 6);
    assert_eq!(entries[2].seq, 7);
}

#[test]
fn test_scan_beyond_end() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    for i in 1..=5 {
        let tx = create_test_tx(agent_id);
        store.enqueue_tx(&tx).unwrap();
        let (token, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
        let entry = RecordEntry::builder(i, tx).build();
        store
            .append_entry_atomic(agent_id, i, &entry, token.inbox_seq())
            .unwrap();
    }

    let entries = store.scan_record(agent_id, 3, 100).unwrap();
    assert_eq!(entries.len(), 3); // Only entries 3, 4, 5
}

#[test]
fn test_get_nonexistent_entry() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let result = store.get_record_entry(agent_id, 999);
    assert!(matches!(
        result,
        Err(StoreError::RecordEntryNotFound(_, 999))
    ));
}

#[test]
fn test_transaction_payload_preserved() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let payload = b"complex payload with \x00 null bytes and unicode: \xC3\xA9";
    let tx = Transaction::new(
        Hash::from_content(payload),
        agent_id,
        1000,
        TransactionType::UserPrompt,
        Bytes::from(payload.to_vec()),
    );

    store.enqueue_tx(&tx).unwrap();
    let (_, dequeued_tx) = store.dequeue_tx(agent_id).unwrap().unwrap();

    assert_eq!(dequeued_tx.payload.as_ref(), payload);
}

#[test]
fn test_record_entry_with_complex_data() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let tx = create_test_tx(agent_id);
    store.enqueue_tx(&tx).unwrap();
    let (token, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();

    let mut decision = Decision::new();
    let action_id = aura_core_types::ActionId::generate();
    decision.accept(action_id);
    decision.reject(0, "test rejection");

    let entry = RecordEntry::builder(1, tx)
        .context_hash([42u8; 32])
        .proposals(ProposalSet::new())
        .decision(decision)
        .build();

    store
        .append_entry_atomic(agent_id, 1, &entry, token.inbox_seq())
        .unwrap();

    let retrieved = store.get_record_entry(agent_id, 1).unwrap();
    assert_eq!(
        retrieved.context_hash,
        aura_core_types::ContextHash::from([42u8; 32])
    );
    assert_eq!(retrieved.decision.accepted_action_ids.len(), 1);
    assert_eq!(retrieved.decision.rejected.len(), 1);
    assert_eq!(retrieved.decision.rejected[0].reason, "test rejection");
}

#[test]
fn test_scan_record_deserialization_error() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let tx = create_test_tx(agent_id);
    store.enqueue_tx(&tx).unwrap();
    let (token, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
    let entry = RecordEntry::builder(1, tx).build();
    store
        .append_entry_atomic(agent_id, 1, &entry, token.inbox_seq())
        .unwrap();

    let record_key = RecordKey::new(agent_id, 1);
    let cf = store.db.cf_handle(cf::RECORD).unwrap();
    store
        .db
        .put_cf(&cf, record_key.encode(), b"not valid json {{{{")
        .unwrap();

    let result = store.scan_record(agent_id, 1, 10);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, StoreError::Deserialization(_)),
        "expected Deserialization, got: {err:?}"
    );
}
