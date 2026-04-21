//! Invariant §7 + §10 — atomic commit of record append + inbox dequeue.
//!
//! This integration test exercises [`RocksStore::append_entry_atomic`]
//! through the fault-injected sibling
//! `append_entry_atomic_with_fault` and asserts that the observable
//! store state after a fault is always one of:
//!
//! * **both** the record entry was appended **and** the inbox slot was
//!   deleted (atomic commit succeeded, caller observed success or a
//!   post-commit caller-side failure), or
//! * **neither** happened (atomic commit never ran).
//!
//! A third "broken non-atomic path" (`FaultAt::InsideBatch`) is
//! exercised explicitly to prove that skipping the single `WriteBatch`
//! produces partial state — this documents why the real
//! `append_entry_atomic` uses one batch.
//!
//! Enforcement targets: Invariant §7 (monotonic sequencing, atomic
//! inbox+record commit) and Invariant §10 (sealed `WriteStore`).

#![cfg(feature = "test-support")]

use aura_core::{AgentId, Decision, ProposalSet, RecordEntry, Transaction};
use aura_store::{FaultAt, ReadStore, RocksStore, Store, StoreError, WriteStore};
use std::sync::Arc;
use tempfile::TempDir;

// Compile-time check: `RocksStore` is the canonical `WriteStore` impl.
// Invariant §10 seals the trait via a crate-private marker
// (`aura_store::store::sealed::Sealed`) so only types declared inside
// `aura-store` can satisfy the bound. A negative compile-fail test is
// redundant because an external `impl WriteStore for MyType {}` would
// be rejected by rustc at build time with a private-trait error —
// instead we positively assert the known-good impl exists.
static_assertions::assert_impl_all!(RocksStore: WriteStore, Store);

fn mk_store() -> (Arc<RocksStore>, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let store = Arc::new(RocksStore::open(dir.path(), false).expect("open rocks"));
    (store, dir)
}

fn seed_inbox_and_build_entry(store: &RocksStore, agent_id: AgentId) -> (RecordEntry, u64) {
    // Push a real transaction through `enqueue_tx` so the inbox has a
    // live slot with seq=0 to consume.
    let tx = Transaction::user_prompt(agent_id, "hello");
    store.enqueue_tx(&tx).expect("enqueue_tx");
    let (token, dequeued_tx) = store
        .dequeue_tx(agent_id)
        .expect("dequeue_tx")
        .expect("inbox not empty");
    assert_eq!(dequeued_tx.hash, tx.hash);

    // Build a matching record entry at next_seq = head_seq + 1.
    let next_seq = store.get_head_seq(agent_id).expect("head_seq") + 1;
    let entry = RecordEntry::builder(next_seq, tx)
        .context_hash([0u8; 32])
        .proposals(ProposalSet::new())
        .decision(Decision::new())
        .build();

    (entry, token.inbox_seq())
}

#[test]
fn pre_batch_fault_leaves_record_and_inbox_untouched() {
    let (store, _dir) = mk_store();
    let agent_id = AgentId::generate();
    let (entry, inbox_seq) = seed_inbox_and_build_entry(&store, agent_id);

    let pre_head = store.get_head_seq(agent_id).expect("head_seq");
    let pre_depth = store.get_inbox_depth(agent_id).expect("inbox depth");

    let err = store
        .append_entry_atomic_with_fault(
            agent_id,
            entry.seq,
            &entry,
            inbox_seq,
            FaultAt::BeforeBatchWrite,
        )
        .expect_err("fault must surface as an error");
    assert!(matches!(err, StoreError::InvalidKey(ref msg) if msg.contains("BeforeBatchWrite")));

    // Invariant §10 pre-commit consistency: neither surface moved.
    assert_eq!(store.get_head_seq(agent_id).unwrap(), pre_head);
    assert_eq!(store.get_inbox_depth(agent_id).unwrap(), pre_depth);
    assert!(store
        .get_record_entry(agent_id, entry.seq)
        .is_err_and(|e| matches!(e, StoreError::RecordEntryNotFound(..))));
}

#[test]
fn post_batch_fault_commits_atomically_even_on_caller_error() {
    let (store, _dir) = mk_store();
    let agent_id = AgentId::generate();
    let (entry, inbox_seq) = seed_inbox_and_build_entry(&store, agent_id);

    let err = store
        .append_entry_atomic_with_fault(
            agent_id,
            entry.seq,
            &entry,
            inbox_seq,
            FaultAt::AfterBatchWrite,
        )
        .expect_err("fault must surface as an error");
    assert!(matches!(err, StoreError::InvalidKey(ref msg) if msg.contains("AfterBatchWrite")));

    // The atomic batch was committed before the caller-side failure —
    // both sides of the atomic pair landed.
    assert_eq!(store.get_head_seq(agent_id).unwrap(), entry.seq);
    let persisted = store.get_record_entry(agent_id, entry.seq).unwrap();
    assert_eq!(persisted.seq, entry.seq);
    assert_eq!(
        store.get_inbox_depth(agent_id).unwrap(),
        0,
        "inbox slot must have been consumed atomically with the record append"
    );
}

#[test]
fn success_path_appends_record_and_consumes_inbox() {
    let (store, _dir) = mk_store();
    let agent_id = AgentId::generate();
    let (entry, inbox_seq) = seed_inbox_and_build_entry(&store, agent_id);

    store
        .append_entry_atomic(agent_id, entry.seq, &entry, inbox_seq)
        .expect("atomic append must succeed");

    assert_eq!(store.get_head_seq(agent_id).unwrap(), entry.seq);
    assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 0);
    let persisted = store.get_record_entry(agent_id, entry.seq).unwrap();
    assert_eq!(persisted.seq, entry.seq);
}

#[test]
fn broken_non_atomic_path_produces_partial_state() {
    // This test does NOT assert the consistency invariant — it is the
    // counter-example that justifies why `append_entry_atomic` bundles
    // the record put and the inbox delete into a single `WriteBatch`.
    // Observing partial state here (record appended, inbox still
    // populated) is the expected failure mode of the hypothetical
    // non-atomic implementation, and it is precisely what the real
    // atomic path prevents.
    let (store, _dir) = mk_store();
    let agent_id = AgentId::generate();
    let (entry, inbox_seq) = seed_inbox_and_build_entry(&store, agent_id);

    let err = store
        .append_entry_atomic_with_fault(
            agent_id,
            entry.seq,
            &entry,
            inbox_seq,
            FaultAt::InsideBatch,
        )
        .expect_err("broken path must surface as an error");
    assert!(matches!(err, StoreError::InvalidKey(ref msg) if msg.contains("InsideBatch")));

    // Partial state: record landed, inbox did NOT advance. This is the
    // bug the atomic path prevents.
    assert_eq!(store.get_head_seq(agent_id).unwrap(), entry.seq);
    assert!(store.get_record_entry(agent_id, entry.seq).is_ok());
    assert_eq!(
        store.get_inbox_depth(agent_id).unwrap(),
        1,
        "the broken non-atomic path must leave the inbox slot behind"
    );
}

#[test]
fn sequence_mismatch_does_not_mutate_store() {
    // Invariant §7: `append_entry_atomic` rejects sequence mismatches.
    // Verify that rejection is a pure read — no partial state change.
    let (store, _dir) = mk_store();
    let agent_id = AgentId::generate();
    let (entry, inbox_seq) = seed_inbox_and_build_entry(&store, agent_id);

    let bad_entry = RecordEntry::builder(entry.seq + 5, entry.tx.clone())
        .context_hash([1u8; 32])
        .build();
    let err = store
        .append_entry_atomic(agent_id, bad_entry.seq, &bad_entry, inbox_seq)
        .expect_err("non-contiguous seq must fail");
    assert!(matches!(err, StoreError::SequenceMismatch { .. }));

    assert_eq!(store.get_head_seq(agent_id).unwrap(), 0);
    assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 1);
}
