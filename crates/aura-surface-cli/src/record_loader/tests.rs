use super::*;
use aura_store_db::{RocksStore, WriteStore};

fn append(store: &RocksStore, seq: u64, tx: Transaction) {
    let agent_id = tx.agent_id;
    let entry = RecordEntry::builder(seq, tx).build();
    store.append_entry_direct(agent_id, seq, &entry).unwrap();
}

fn text_tx(agent_id: AgentId, tx_type: TransactionType, text: &str) -> Transaction {
    Transaction::new_chained(agent_id, tx_type, text.as_bytes().to_vec(), None)
}

#[test]
fn conversation_history_rebuilds_chat_per_agent() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let alice = AgentId::generate();
    let bob = AgentId::generate();

    append(&store, 1, Transaction::session_start(alice));
    append(
        &store,
        2,
        text_tx(alice, TransactionType::UserPrompt, "hi alice"),
    );
    append(
        &store,
        3,
        text_tx(alice, TransactionType::ToolExecution, "not json"),
    );
    append(
        &store,
        4,
        text_tx(alice, TransactionType::AgentMsg, "hello from alice"),
    );
    append(&store, 5, text_tx(alice, TransactionType::AgentMsg, ""));
    append(
        &store,
        1,
        text_tx(bob, TransactionType::UserPrompt, "hi bob"),
    );

    let history = conversation_history(&store, alice).unwrap();
    let rendered: Vec<(MessageRole, &str)> = history
        .iter()
        .map(|m| (m.role, m.content.as_str()))
        .collect();
    assert_eq!(
        rendered,
        [
            (MessageRole::System, "── new session ──"),
            (MessageRole::User, "hi alice"),
            (MessageRole::Assistant, "hello from alice"),
        ]
    );

    let bob_history = conversation_history(&store, bob).unwrap();
    assert_eq!(bob_history.len(), 1);
    assert_eq!(bob_history[0].content, "hi bob");

    assert!(conversation_history(&store, AgentId::generate())
        .unwrap()
        .is_empty());
}

#[test]
fn recent_record_summaries_are_scoped_to_agent() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let alice = AgentId::generate();
    let bob = AgentId::generate();

    append(&store, 1, text_tx(alice, TransactionType::UserPrompt, "a1"));
    append(&store, 2, text_tx(alice, TransactionType::AgentMsg, "a2"));
    append(&store, 1, text_tx(bob, TransactionType::UserPrompt, "b1"));

    let summaries = recent_record_summaries(&store, alice).unwrap();
    let seqs: Vec<u64> = summaries.iter().map(|s| s.seq).collect();
    assert_eq!(seqs, [1, 2]);
    assert!(summaries.iter().all(|s| s.agent_id == alice.to_hex()));
}
