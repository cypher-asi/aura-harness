use super::*;
use aura_core::{
    Decision, Hash, InstalledIntegrationDefinition, InstalledToolCapability,
    InstalledToolIntegrationRequirement, ProposalSet, RuntimeCapabilityInstall, SystemKind,
    TransactionType,
};
use bytes::Bytes;
use std::collections::HashMap;
use tempfile::TempDir;

fn create_test_store() -> (RocksStore, TempDir) {
    let dir = TempDir::new().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    (store, dir)
}

fn create_test_tx(agent_id: AgentId) -> Transaction {
    Transaction::new(
        Hash::from_content(b"test"),
        agent_id,
        1000,
        TransactionType::UserPrompt,
        Bytes::from_static(b"test payload"),
    )
}

#[allow(deprecated)]
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
    assert_eq!(dequeued_tx.tx_id(), tx.tx_id());
}

#[test]
fn test_inbox_empty() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let result = store.dequeue_tx(agent_id).unwrap();
    assert!(result.is_none());
}

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
}

#[test]
fn test_agent_status() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let status = store.get_agent_status(agent_id).unwrap();
    assert_eq!(status, AgentStatus::Active);

    store
        .set_agent_status(agent_id, AgentStatus::Paused)
        .unwrap();
    let status = store.get_agent_status(agent_id).unwrap();
    assert_eq!(status, AgentStatus::Paused);
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

// ========================================================================
// Edge Case Tests
// ========================================================================

#[test]
fn test_empty_agent_state() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    assert_eq!(store.get_head_seq(agent_id).unwrap(), 0);
    assert!(!store.has_pending_tx(agent_id).unwrap());
    assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 0);
    assert!(store.dequeue_tx(agent_id).unwrap().is_none());
}

#[test]
fn test_multiple_agents_isolated() {
    let (store, _dir) = create_test_store();

    let agent1 = AgentId::generate();
    let agent2 = AgentId::generate();

    let tx1 = create_test_tx(agent1);
    let tx2 = create_test_tx(agent2);

    store.enqueue_tx(&tx1).unwrap();
    store.enqueue_tx(&tx2).unwrap();

    assert_eq!(store.get_inbox_depth(agent1).unwrap(), 1);
    assert_eq!(store.get_inbox_depth(agent2).unwrap(), 1);

    let (token, tx) = store.dequeue_tx(agent1).unwrap().unwrap();
    let entry = RecordEntry::builder(1, tx).build();
    store
        .append_entry_atomic(agent1, 1, &entry, token.inbox_seq())
        .unwrap();

    assert_eq!(store.get_head_seq(agent1).unwrap(), 1);
    assert_eq!(store.get_head_seq(agent2).unwrap(), 0);
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
fn test_agent_status_transitions() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    assert_eq!(
        store.get_agent_status(agent_id).unwrap(),
        AgentStatus::Active
    );

    store
        .set_agent_status(agent_id, AgentStatus::Paused)
        .unwrap();
    assert_eq!(
        store.get_agent_status(agent_id).unwrap(),
        AgentStatus::Paused
    );

    store.set_agent_status(agent_id, AgentStatus::Dead).unwrap();
    assert_eq!(store.get_agent_status(agent_id).unwrap(), AgentStatus::Dead);

    store
        .set_agent_status(agent_id, AgentStatus::Active)
        .unwrap();
    assert_eq!(
        store.get_agent_status(agent_id).unwrap(),
        AgentStatus::Active
    );
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
    let action_id = aura_core::ActionId::generate();
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
    assert_eq!(retrieved.context_hash, [42u8; 32]);
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

// ========================================================================
// Direct Append Tests (no inbox coupling)
// ========================================================================

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
    assert_eq!(retrieved.context_hash, [1u8; 32]);
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

// ========================================================================
// Batch Append Tests
// ========================================================================

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
    assert_eq!(retrieved.context_hash, [1u8; 32]);
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
        assert_eq!(entry.context_hash, [(i + 1) as u8; 32]);
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

// ========================================================================
// DequeueToken + append_entry_dequeued Tests
// ========================================================================

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

    assert_eq!(store.get_runtime_capabilities(agent_id).unwrap(), Some(runtime_capabilities));
    assert!(!store.has_pending_tx(agent_id).unwrap());
}

#[test]
fn test_runtime_capabilities_round_trip() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();
    let tx = create_test_tx(agent_id);
    let entry = RecordEntry::builder(1, tx).build();
    let runtime_capabilities = RuntimeCapabilityInstall {
        system_kind: SystemKind::CapabilityInstall,
        scope: "session".to_string(),
        session_id: Some("session-1".to_string()),
        installed_integrations: vec![InstalledIntegrationDefinition {
            integration_id: "integration-brave-1".to_string(),
            name: "Brave Search".to_string(),
            provider: "brave_search".to_string(),
            kind: "workspace_integration".to_string(),
            metadata: HashMap::new(),
        }],
        installed_tools: vec![InstalledToolCapability {
            name: "brave_search_web".to_string(),
            required_integration: Some(InstalledToolIntegrationRequirement {
                integration_id: None,
                provider: Some("brave_search".to_string()),
                kind: Some("workspace_integration".to_string()),
            }),
        }],
    };

    store
        .append_entry_direct_with_runtime_capabilities(
            agent_id,
            1,
            &entry,
            Some(&runtime_capabilities),
            false,
        )
        .unwrap();

    let persisted = store.get_runtime_capabilities(agent_id).unwrap();
    assert_eq!(persisted, Some(runtime_capabilities));
}

#[test]
fn test_runtime_capabilities_can_be_cleared_atomically() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let entry1 = RecordEntry::builder(1, create_test_tx(agent_id)).build();
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
        .append_entry_direct_with_runtime_capabilities(
            agent_id,
            1,
            &entry1,
            Some(&runtime_capabilities),
            false,
        )
        .unwrap();
    assert!(store.get_runtime_capabilities(agent_id).unwrap().is_some());

    let entry2 = RecordEntry::builder(2, create_test_tx(agent_id)).build();
    store
        .append_entry_direct_with_runtime_capabilities(agent_id, 2, &entry2, None, true)
        .unwrap();

    assert_eq!(store.get_runtime_capabilities(agent_id).unwrap(), None);
}
