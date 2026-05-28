//! Integration test for the [`aura_store_record::RecordLog`] bridge
//! over [`aura_store_db::RocksStore`].
//!
//! Verifies the Phase 2 contract: a `&dyn RecordLog` over a real
//! `RocksStore` can append a sequence of entries for one agent and
//! report a head_seq matching the number of entries appended.

use aura_core::{AgentId, Decision, ProposalSet, RecordEntry, Transaction};
use aura_store_db::RocksStore;
use aura_store_record::{RecordLog, RecordLogError};
use tempfile::TempDir;

fn make_entry(agent_id: AgentId, seq: u64) -> RecordEntry {
    let tx = Transaction::user_prompt(agent_id, format!("entry {seq}").into_bytes());
    RecordEntry::builder(seq, tx)
        .context_hash([0u8; 32])
        .proposals(ProposalSet::new())
        .decision(Decision::new())
        .actions(vec![])
        .effects(vec![])
        .build()
}

#[test]
fn record_log_bridge_appends_three_entries_and_reports_head_seq_three() {
    let tmp = TempDir::new().expect("create tempdir");
    let store = RocksStore::open(tmp.path(), false).expect("open RocksStore");

    let agent_id = AgentId::generate();
    let log: &dyn RecordLog = &store;

    assert_eq!(
        log.head_seq(&agent_id)
            .expect("head_seq before any appends"),
        0,
        "fresh agent must report head_seq=0"
    );

    for seq in 1..=3 {
        let entry = make_entry(agent_id, seq);
        log.append(&agent_id, &entry)
            .unwrap_or_else(|e| panic!("append seq={seq} should succeed: {e}"));
    }

    assert_eq!(
        log.head_seq(&agent_id)
            .expect("head_seq after three appends"),
        3,
        "head_seq should advance to the last appended sequence"
    );
}

#[test]
fn record_log_bridge_rejects_out_of_order_seq() {
    let tmp = TempDir::new().expect("create tempdir");
    let store = RocksStore::open(tmp.path(), false).expect("open RocksStore");

    let agent_id = AgentId::generate();
    let log: &dyn RecordLog = &store;

    let entry_skipping_one = make_entry(agent_id, 5);
    let err = log
        .append(&agent_id, &entry_skipping_one)
        .expect_err("append at seq=5 with no prior must fail");

    match err {
        RecordLogError::SeqOutOfOrder {
            expected, actual, ..
        } => {
            assert_eq!(expected, 1);
            assert_eq!(actual, 5);
        }
        RecordLogError::Backend(msg) => panic!("expected SeqOutOfOrder, got Backend: {msg}"),
    }

    assert_eq!(
        log.head_seq(&agent_id)
            .expect("head_seq after rejected append"),
        0,
        "rejected append must not advance head_seq"
    );
}

#[test]
fn record_log_bridge_works_through_arc() {
    use std::sync::Arc;

    let tmp = TempDir::new().expect("create tempdir");
    let store = Arc::new(RocksStore::open(tmp.path(), false).expect("open RocksStore"));
    let agent_id = AgentId::generate();

    let log: Arc<dyn RecordLog> = store.clone();
    let entry = make_entry(agent_id, 1);
    log.append(&agent_id, &entry).expect("append via Arc<dyn>");
    assert_eq!(log.head_seq(&agent_id).expect("head_seq via Arc<dyn>"), 1);
}
