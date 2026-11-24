//! `append_replica_batch`: the cloud record mirror's write path. Entries
//! must continue the local head with contiguous seqs, and a refused
//! batch writes nothing.

use aura_core_types::{AgentId, RecordEntry, Transaction};
use aura_store_db::{ReadStore, RocksStore, StoreError, WriteStore};
use tempfile::TempDir;

fn store() -> (RocksStore, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let store = RocksStore::open(dir.path(), false).expect("open rocks");
    (store, dir)
}

fn entry(agent: AgentId, seq: u64) -> RecordEntry {
    RecordEntry::builder(
        seq,
        Transaction::user_prompt(agent, format!("prompt {seq}")),
    )
    .build()
}

#[test]
fn a_contiguous_page_is_appended_verbatim() {
    let (store, _dir) = store();
    let agent = AgentId::new([4; 32]);
    let page = vec![entry(agent, 1), entry(agent, 2), entry(agent, 3)];
    store.append_replica_batch(agent, 1, &page).expect("append");
    assert_eq!(store.get_head_seq(agent).expect("head"), 3);
    assert_eq!(store.get_record_entry(agent, 2).expect("entry"), page[1]);
}

#[test]
fn a_gap_inside_the_page_writes_nothing() {
    let (store, _dir) = store();
    let agent = AgentId::new([4; 32]);
    let page = vec![entry(agent, 1), entry(agent, 3)];
    let err = store
        .append_replica_batch(agent, 1, &page)
        .expect_err("gap");
    assert!(
        matches!(
            err,
            StoreError::SequenceMismatch {
                expected: 2,
                actual: 3
            }
        ),
        "{err:?}"
    );
    assert_eq!(store.get_head_seq(agent).expect("head"), 0);
}

#[test]
fn a_page_that_does_not_continue_the_head_is_refused() {
    let (store, _dir) = store();
    let agent = AgentId::new([4; 32]);
    store
        .append_replica_batch(agent, 1, &[entry(agent, 1)])
        .expect("first");
    let err = store
        .append_replica_batch(agent, 1, &[entry(agent, 1)])
        .expect_err("duplicate");
    assert!(
        matches!(err, StoreError::SequenceMismatch { .. }),
        "{err:?}"
    );
    assert_eq!(store.get_head_seq(agent).expect("head"), 1);
}
