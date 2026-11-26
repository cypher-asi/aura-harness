//! Appending record entries: atomic commits, direct and batched.

use super::support::*;
use crate::error::StoreError;
use crate::store::ReadStore;
use crate::store::WriteStore;
use aura_core_types::{AgentId, RecordEntry};
use aura_core_types::{
    Decision, InstalledToolCapability, ProposalSet, RuntimeCapabilityInstall, SystemKind,
};

#[test]
fn test_atomic_commit() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();
    let tx = create_test_tx(agent_id);

    store.enqueue_tx(&tx).unwrap();

    let head_seq = store.get_head_seq(agent_id).unwrap();
    assert_eq!(head_seq, 0);

    let (token, _) = store.dequeue_tx(agent_id).unwrap().unwrap();

    let entry = RecordEntry::builder(1, tx)
        .context_hash([0u8; 32])
        .proposals(ProposalSet::new())
        .decision(Decision::new())
        .build();

    store
        .append_entry_atomic(agent_id, 1, &entry, token.inbox_seq())
        .unwrap();

    let new_head = store.get_head_seq(agent_id).unwrap();
    assert_eq!(new_head, 1);

    assert!(!store.has_pending_tx(agent_id).unwrap());

    let retrieved = store.get_record_entry(agent_id, 1).unwrap();
    assert_eq!(retrieved.seq, 1);
}

#[test]
fn test_sequence_mismatch() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();
    let tx = create_test_tx(agent_id);

    store.enqueue_tx(&tx).unwrap();
    let (token, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();

    let entry = RecordEntry::builder(5, tx) // Wrong seq - should be 1
        .build();

    let result = store.append_entry_atomic(agent_id, 5, &entry, token.inbox_seq());
    assert!(matches!(result, Err(StoreError::SequenceMismatch { .. })));
}

#[test]
fn test_append_entry_direct() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let tx = create_test_tx(agent_id);
    let entry = RecordEntry::builder(1, tx).context_hash([1u8; 32]).build();

    store.append_entry_direct(agent_id, 1, &entry).unwrap();

    assert_eq!(store.get_head_seq(agent_id).unwrap(), 1);
    let retrieved = store.get_record_entry(agent_id, 1).unwrap();
    assert_eq!(retrieved.seq, 1);
    assert_eq!(
        retrieved.context_hash,
        aura_core_types::ContextHash::from([1u8; 32])
    );
}

#[test]
fn test_append_entry_direct_sequence_mismatch() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let tx = create_test_tx(agent_id);
    let entry = RecordEntry::builder(5, tx).build();

    let result = store.append_entry_direct(agent_id, 5, &entry);
    assert!(matches!(result, Err(StoreError::SequenceMismatch { .. })));
}

#[test]
fn test_append_entry_direct_multiple() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    for i in 1..=5 {
        let tx = create_test_tx(agent_id);
        let entry = RecordEntry::builder(i, tx)
            .context_hash([i as u8; 32])
            .build();
        store.append_entry_direct(agent_id, i, &entry).unwrap();
    }

    assert_eq!(store.get_head_seq(agent_id).unwrap(), 5);
    let entries = store.scan_record(agent_id, 1, 10).unwrap();
    assert_eq!(entries.len(), 5);
}

#[test]
fn test_append_entry_direct_does_not_touch_inbox() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let tx = create_test_tx(agent_id);
    store.enqueue_tx(&tx).unwrap();
    assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 1);

    let entry_tx = create_test_tx(agent_id);
    let entry = RecordEntry::builder(1, entry_tx).build();
    store.append_entry_direct(agent_id, 1, &entry).unwrap();

    assert_eq!(
        store.get_inbox_depth(agent_id).unwrap(),
        1,
        "Direct append must not drain inbox"
    );
}

#[test]
fn test_append_entries_batch_empty() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    store.append_entries_batch(agent_id, 1, &[]).unwrap();
    assert_eq!(store.get_head_seq(agent_id).unwrap(), 0);
}

#[test]
fn test_append_entries_batch_single() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let tx = create_test_tx(agent_id);
    let entry = RecordEntry::builder(1, tx).context_hash([1u8; 32]).build();

    store.append_entries_batch(agent_id, 1, &[entry]).unwrap();

    assert_eq!(store.get_head_seq(agent_id).unwrap(), 1);
    let retrieved = store.get_record_entry(agent_id, 1).unwrap();
    assert_eq!(
        retrieved.context_hash,
        aura_core_types::ContextHash::from([1u8; 32])
    );
}

#[test]
fn test_append_entries_batch_multiple() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let entries: Vec<RecordEntry> = (1..=5)
        .map(|i| {
            let tx = create_test_tx(agent_id);
            RecordEntry::builder(i, tx)
                .context_hash([i as u8; 32])
                .build()
        })
        .collect();

    store.append_entries_batch(agent_id, 1, &entries).unwrap();

    assert_eq!(store.get_head_seq(agent_id).unwrap(), 5);
    let scanned = store.scan_record(agent_id, 1, 10).unwrap();
    assert_eq!(scanned.len(), 5);
    for (i, entry) in scanned.iter().enumerate() {
        assert_eq!(entry.seq, (i + 1) as u64);
        assert_eq!(
            entry.context_hash,
            aura_core_types::ContextHash::from([(i + 1) as u8; 32])
        );
    }
}

#[test]
fn test_append_entries_batch_sequence_mismatch() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let tx = create_test_tx(agent_id);
    let entry = RecordEntry::builder(5, tx).build();

    let result = store.append_entries_batch(agent_id, 5, &[entry]);
    assert!(matches!(result, Err(StoreError::SequenceMismatch { .. })));
}

#[test]
fn test_append_entries_batch_continues_from_existing() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let tx1 = create_test_tx(agent_id);
    let entry1 = RecordEntry::builder(1, tx1).build();
    store.append_entry_direct(agent_id, 1, &entry1).unwrap();

    let entries: Vec<RecordEntry> = (2..=4)
        .map(|i| {
            let tx = create_test_tx(agent_id);
            RecordEntry::builder(i, tx).build()
        })
        .collect();
    store.append_entries_batch(agent_id, 2, &entries).unwrap();

    assert_eq!(store.get_head_seq(agent_id).unwrap(), 4);
    let scanned = store.scan_record(agent_id, 1, 10).unwrap();
    assert_eq!(scanned.len(), 4);
}

#[test]
fn test_append_entry_dequeued() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let tx = create_test_tx(agent_id);
    store.enqueue_tx(&tx).unwrap();

    let (token, dequeued_tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
    assert_eq!(token.inbox_seq(), 0);

    let entry = RecordEntry::builder(1, dequeued_tx).build();
    store
        .append_entry_dequeued(agent_id, 1, &entry, token)
        .unwrap();

    assert_eq!(store.get_head_seq(agent_id).unwrap(), 1);
    assert!(!store.has_pending_tx(agent_id).unwrap());
}

#[test]
fn test_append_entry_dequeued_with_runtime_capabilities() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();
    let tx = create_test_tx(agent_id);
    store.enqueue_tx(&tx).unwrap();

    let (token, dequeued_tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
    let entry = RecordEntry::builder(1, dequeued_tx).build();
    let runtime_capabilities = RuntimeCapabilityInstall {
        system_kind: SystemKind::CapabilityInstall,
        scope: "session".to_string(),
        session_id: Some("session-1".to_string()),
        installed_integrations: vec![],
        installed_tools: vec![InstalledToolCapability {
            name: "brave_search_web".to_string(),
            required_integration: None,
        }],
    };

    store
        .append_entry_dequeued_with_runtime_capabilities(
            agent_id,
            1,
            &entry,
            token,
            Some(&runtime_capabilities),
            false,
        )
        .unwrap();

    assert_eq!(
        store.get_runtime_capabilities(agent_id).unwrap(),
        Some(runtime_capabilities)
    );
    assert!(!store.has_pending_tx(agent_id).unwrap());
}
