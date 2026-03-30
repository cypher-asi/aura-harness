use super::*;
use aura_core::{Hash, TransactionType};
use bytes::Bytes;
use std::sync::Arc;
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

// ========================================================================
// Concurrency Tests (single-threaded simulation)
// ========================================================================

#[test]
fn test_interleaved_agent_operations() {
    let (store, _dir) = create_test_store();

    let agents: Vec<AgentId> = (0..5).map(|_| AgentId::generate()).collect();

    for round in 0..3 {
        for agent in &agents {
            let tx = create_test_tx(*agent);
            store.enqueue_tx(&tx).unwrap();
        }

        for agent in &agents {
            let (inbox_seq, tx) = store.dequeue_tx(*agent).unwrap().unwrap();
            let seq = round as u64 + 1;
            let entry = RecordEntry::builder(seq, tx).build();
            store
                .append_entry_atomic(*agent, seq, &entry, inbox_seq)
                .unwrap();
        }
    }

    for agent in &agents {
        assert_eq!(store.get_head_seq(*agent).unwrap(), 3);
    }
}

#[test]
fn test_reopen_store() {
    let dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();

    {
        let store = RocksStore::open(dir.path(), false).unwrap();

        let tx = create_test_tx(agent_id);
        store.enqueue_tx(&tx).unwrap();
        let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
        let entry = RecordEntry::builder(1, tx).build();
        store
            .append_entry_atomic(agent_id, 1, &entry, inbox_seq)
            .unwrap();

        store
            .set_agent_status(agent_id, AgentStatus::Paused)
            .unwrap();
    }

    {
        let store = RocksStore::open(dir.path(), false).unwrap();

        assert_eq!(store.get_head_seq(agent_id).unwrap(), 1);
        assert_eq!(
            store.get_agent_status(agent_id).unwrap(),
            AgentStatus::Paused
        );

        let entry = store.get_record_entry(agent_id, 1).unwrap();
        assert_eq!(entry.seq, 1);
    }
}

// ========================================================================
// Concurrent Read/Write Tests
// ========================================================================

#[tokio::test]
async fn test_concurrent_writes_different_agents() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(RocksStore::open(dir.path(), false).unwrap());

    let mut handles = Vec::new();
    for _ in 0..10 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let agent_id = AgentId::generate();
            let tx = Transaction::new(
                Hash::from_content(b"concurrent"),
                agent_id,
                1000,
                TransactionType::UserPrompt,
                Bytes::from_static(b"test payload"),
            );
            store.enqueue_tx(&tx).unwrap();
            let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
            let entry = RecordEntry::builder(1, tx).build();
            store
                .append_entry_atomic(agent_id, 1, &entry, inbox_seq)
                .unwrap();
            agent_id
        }));
    }

    let agent_ids: Vec<AgentId> = futures_util::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    for agent_id in &agent_ids {
        assert_eq!(store.get_head_seq(*agent_id).unwrap(), 1);
    }
}

#[tokio::test]
async fn test_concurrent_reads_and_writes() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(RocksStore::open(dir.path(), false).unwrap());
    let agent_id = AgentId::generate();

    for i in 1..=5 {
        let tx = create_test_tx(agent_id);
        store.enqueue_tx(&tx).unwrap();
        let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
        let entry = RecordEntry::builder(i, tx).build();
        store
            .append_entry_atomic(agent_id, i, &entry, inbox_seq)
            .unwrap();
    }

    let mut handles = Vec::new();

    for _ in 0..5 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let entries = store.scan_record(agent_id, 1, 10).unwrap();
            assert_eq!(entries.len(), 5);
            let head = store.get_head_seq(agent_id).unwrap();
            assert_eq!(head, 5);
        }));
    }

    let store_w = Arc::clone(&store);
    handles.push(tokio::spawn(async move {
        let other_agent = AgentId::generate();
        for i in 1..=3 {
            let tx = Transaction::new(
                Hash::from_content(format!("other-{i}").as_bytes()),
                other_agent,
                1000,
                TransactionType::UserPrompt,
                Bytes::from(format!("payload-{i}")),
            );
            store_w.enqueue_tx(&tx).unwrap();
            let (inbox_seq, tx) = store_w.dequeue_tx(other_agent).unwrap().unwrap();
            let entry = RecordEntry::builder(i, tx).build();
            store_w
                .append_entry_atomic(other_agent, i, &entry, inbox_seq)
                .unwrap();
        }
    }));

    futures_util::future::join_all(handles)
        .await
        .into_iter()
        .for_each(|r| r.unwrap());
}

#[tokio::test]
async fn test_concurrent_enqueue_same_agent() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(RocksStore::open(dir.path(), false).unwrap());
    let agent_id = AgentId::generate();

    let mut handles = Vec::new();
    for i in 0..10u64 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let tx = Transaction::new(
                Hash::from_content(format!("tx-{i}").as_bytes()),
                agent_id,
                1000 + i,
                TransactionType::UserPrompt,
                Bytes::from(format!("payload-{i}")),
            );
            store.enqueue_tx(&tx).unwrap();
        }));
    }

    futures_util::future::join_all(handles)
        .await
        .into_iter()
        .for_each(|r| r.unwrap());

    assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 10);
}

// ========================================================================
// Crash-Recovery Simulation Tests
// ========================================================================

#[test]
fn test_crash_recovery_inbox_persists() {
    let dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();

    {
        let store = RocksStore::open(dir.path(), false).unwrap();
        for _ in 0..3 {
            let tx = create_test_tx(agent_id);
            store.enqueue_tx(&tx).unwrap();
        }
        assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 3);
    }

    {
        let store = RocksStore::open(dir.path(), false).unwrap();
        assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 3);

        let (inbox_seq, _) = store.dequeue_tx(agent_id).unwrap().unwrap();
        assert_eq!(inbox_seq, 0);
    }
}

#[test]
fn test_crash_recovery_record_entries_persist() {
    let dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();

    {
        let store = RocksStore::open(dir.path(), false).unwrap();
        for i in 1..=5 {
            let tx = create_test_tx(agent_id);
            store.enqueue_tx(&tx).unwrap();
            let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
            let entry = RecordEntry::builder(i, tx)
                .context_hash([i as u8; 32])
                .build();
            store
                .append_entry_atomic(agent_id, i, &entry, inbox_seq)
                .unwrap();
        }
    }

    {
        let store = RocksStore::open(dir.path(), false).unwrap();
        assert_eq!(store.get_head_seq(agent_id).unwrap(), 5);

        let entries = store.scan_record(agent_id, 1, 10).unwrap();
        assert_eq!(entries.len(), 5);
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(entry.seq, (i + 1) as u64);
            assert_eq!(entry.context_hash, [(i + 1) as u8; 32]);
        }
    }
}

#[test]
fn test_crash_recovery_multiple_agents() {
    let dir = TempDir::new().unwrap();
    let agents: Vec<AgentId> = (0..3).map(|_| AgentId::generate()).collect();

    {
        let store = RocksStore::open(dir.path(), false).unwrap();
        for (idx, agent_id) in agents.iter().enumerate() {
            for i in 1..=((idx + 1) as u64) {
                let tx = create_test_tx(*agent_id);
                store.enqueue_tx(&tx).unwrap();
                let (inbox_seq, tx) = store.dequeue_tx(*agent_id).unwrap().unwrap();
                let entry = RecordEntry::builder(i, tx).build();
                store
                    .append_entry_atomic(*agent_id, i, &entry, inbox_seq)
                    .unwrap();
            }
            store
                .set_agent_status(*agent_id, AgentStatus::Paused)
                .unwrap();
        }
    }

    {
        let store = RocksStore::open(dir.path(), false).unwrap();
        for (idx, agent_id) in agents.iter().enumerate() {
            let expected_seq = (idx + 1) as u64;
            assert_eq!(store.get_head_seq(*agent_id).unwrap(), expected_seq);
            assert_eq!(
                store.get_agent_status(*agent_id).unwrap(),
                AgentStatus::Paused
            );
        }
    }
}

// ========================================================================
// Additional Scan Edge Case Tests
// ========================================================================

#[test]
fn test_scan_single_entry() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    let tx = create_test_tx(agent_id);
    store.enqueue_tx(&tx).unwrap();
    let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
    let entry = RecordEntry::builder(1, tx).build();
    store
        .append_entry_atomic(agent_id, 1, &entry, inbox_seq)
        .unwrap();

    let entries = store.scan_record(agent_id, 1, 1).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].seq, 1);
}

#[test]
fn test_scan_with_large_limit() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    for i in 1..=20 {
        let tx = create_test_tx(agent_id);
        store.enqueue_tx(&tx).unwrap();
        let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
        let entry = RecordEntry::builder(i, tx).build();
        store
            .append_entry_atomic(agent_id, i, &entry, inbox_seq)
            .unwrap();
    }

    let entries = store.scan_record(agent_id, 1, 100_000).unwrap();
    assert_eq!(entries.len(), 20);
}

#[test]
fn test_scan_from_seq_zero() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    for i in 1..=3 {
        let tx = create_test_tx(agent_id);
        store.enqueue_tx(&tx).unwrap();
        let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
        let entry = RecordEntry::builder(i, tx).build();
        store
            .append_entry_atomic(agent_id, i, &entry, inbox_seq)
            .unwrap();
    }

    let entries = store.scan_record(agent_id, 0, 100).unwrap();
    assert_eq!(entries.len(), 3);
}

#[test]
fn test_scan_from_nonexistent_seq() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    for i in 1..=3 {
        let tx = create_test_tx(agent_id);
        store.enqueue_tx(&tx).unwrap();
        let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
        let entry = RecordEntry::builder(i, tx).build();
        store
            .append_entry_atomic(agent_id, i, &entry, inbox_seq)
            .unwrap();
    }

    let entries = store.scan_record(agent_id, 999, 100).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn test_scan_limit_one_returns_single_entry() {
    let (store, _dir) = create_test_store();
    let agent_id = AgentId::generate();

    for i in 1..=5 {
        let tx = create_test_tx(agent_id);
        store.enqueue_tx(&tx).unwrap();
        let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
        let entry = RecordEntry::builder(i, tx).build();
        store
            .append_entry_atomic(agent_id, i, &entry, inbox_seq)
            .unwrap();
    }

    let entries = store.scan_record(agent_id, 1, 1).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].seq, 1);
}
