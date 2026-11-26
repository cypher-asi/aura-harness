//! The per-agent transaction inbox.

use super::support::*;
use crate::cf;
use crate::error::StoreError;
use crate::keys::{AgentMetaKey, KeyCodec};
use crate::store::ReadStore;
use crate::store::WriteStore;
use aura_core_types::{AgentId, RecordEntry};

#[test]
fn test_enqueue_dequeue() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();
    let tx = create_test_tx(agent_id);

    store.enqueue_tx(&tx).unwrap();

    let result = store.dequeue_tx(agent_id).unwrap();
    assert!(result.is_some());

    let (token, dequeued_tx) = result.unwrap();
    assert_eq!(token.inbox_seq(), 0);
    assert_eq!(dequeued_tx.hash, tx.hash);
}

#[test]
fn test_inbox_empty() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let result = store.dequeue_tx(agent_id).unwrap();
    assert!(result.is_none());
}

#[test]
fn test_large_inbox_depth() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    for _ in 0..100 {
        let tx = create_test_tx(agent_id);
        store.enqueue_tx(&tx).unwrap();
    }

    assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 100);

    for seq in 1..=100 {
        let (token, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
        let entry = RecordEntry::builder(seq, tx).build();
        store
            .append_entry_atomic(agent_id, seq, &entry, token.inbox_seq())
            .unwrap();
    }

    assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 0);
    assert_eq!(store.get_head_seq(agent_id).unwrap(), 100);
}

#[test]
fn test_dequeue_tx_inbox_corruption() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let head_key = AgentMetaKey::inbox_head(agent_id);
    let tail_key = AgentMetaKey::inbox_tail(agent_id);
    let cf_meta = store.db.cf_handle(cf::AGENT_META).unwrap();

    store
        .db
        .put_cf(&cf_meta, head_key.encode(), 0u64.to_be_bytes())
        .unwrap();
    store
        .db
        .put_cf(&cf_meta, tail_key.encode(), 1u64.to_be_bytes())
        .unwrap();

    let result = store.dequeue_tx(agent_id);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(
            err,
            StoreError::InboxCorruption {
                agent_id: _,
                expected_seq: 0,
            }
        ),
        "expected InboxCorruption, got: {err:?}"
    );
}
