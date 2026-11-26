use crate::checkpoint::{Checkpoints, SessionCheckpoint, SessionUsage};
use crate::history::{model_history, restore};
use aura_core_types::{AgentId, RecordEntry, Transaction, TransactionType};
use aura_model::{ContentBlock, Message, Role};
use aura_store_db::{RocksStore, WriteStore};

fn append(store: &RocksStore, seq: u64, tx: Transaction) {
    let agent = tx.agent_id;
    let entry = RecordEntry::builder(seq, tx).build();
    store.append_entry_direct(agent, seq, &entry).unwrap();
}

fn reply(agent: AgentId, text: &str) -> Transaction {
    Transaction::new_chained(
        agent,
        TransactionType::AgentMsg,
        text.as_bytes().to_vec(),
        None,
    )
}

fn texts(messages: &[Message]) -> Vec<(Role, String)> {
    messages
        .iter()
        .map(|m| (m.role, m.text_content()))
        .collect()
}

#[tokio::test]
async fn restore_adds_newer_turns_and_drops_a_dangling_tool_call() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path().join("db"), false).unwrap();
    let checkpoints = Checkpoints::new(dir.path());
    let agent = AgentId::generate();
    append(&store, 1, Transaction::user_prompt(agent, "first"));
    append(&store, 2, reply(agent, "calling a tool"));
    let call = Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: "t1".into(),
            name: "read_file".into(),
            input: serde_json::json!({}),
        }],
    );
    let saved = SessionCheckpoint::new(
        5,
        2,
        vec![Message::user("first"), call],
        Vec::new(),
        SessionUsage::default(),
    );
    checkpoints.save(agent, &saved).await.unwrap();
    append(&store, 3, Transaction::user_prompt(agent, "second"));
    append(&store, 4, reply(agent, "answer"));

    let restored = restore(&store, &checkpoints, agent).expect("checkpoint applies");
    assert_eq!(restored.started_ms, 5);
    assert_eq!(
        texts(&restored.messages),
        [
            (Role::User, "first".to_string()),
            (Role::User, "second".to_string()),
            (Role::Assistant, "answer".to_string()),
        ]
    );
}

#[tokio::test]
async fn a_session_start_after_the_checkpoint_ends_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path().join("db"), false).unwrap();
    let checkpoints = Checkpoints::new(dir.path());
    let agent = AgentId::generate();
    append(&store, 1, Transaction::user_prompt(agent, "old"));
    let saved = SessionCheckpoint::new(
        5,
        1,
        vec![Message::user("old")],
        Vec::new(),
        SessionUsage::default(),
    );
    checkpoints.save(agent, &saved).await.unwrap();
    append(&store, 2, Transaction::session_start(agent));
    append(&store, 3, Transaction::user_prompt(agent, "fresh"));

    assert!(restore(&store, &checkpoints, agent).is_none());
    assert_eq!(
        texts(&model_history(&store, agent).unwrap()),
        [(Role::User, "fresh".to_string())]
    );
}

#[tokio::test]
async fn a_checkpoint_ahead_of_the_record_does_not_apply() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path().join("db"), false).unwrap();
    let checkpoints = Checkpoints::new(dir.path());
    let agent = AgentId::generate();
    append(&store, 1, Transaction::user_prompt(agent, "only"));
    let saved = SessionCheckpoint::new(
        5,
        9,
        vec![Message::user("only")],
        Vec::new(),
        SessionUsage::default(),
    );
    checkpoints.save(agent, &saved).await.unwrap();

    assert!(restore(&store, &checkpoints, agent).is_none());
}
