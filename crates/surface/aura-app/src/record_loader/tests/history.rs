use super::*;

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
fn model_history_resumes_the_conversation_since_the_last_session_start() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    let entries = [
        text_tx(agent, TransactionType::UserPrompt, "old question"),
        text_tx(agent, TransactionType::AgentMsg, "old answer"),
        Transaction::session_start(agent),
        text_tx(agent, TransactionType::UserPrompt, "make an app"),
        text_tx(agent, TransactionType::ToolExecution, "not json"),
        text_tx(agent, TransactionType::AgentMsg, "Which stack?"),
        text_tx(agent, TransactionType::UserPrompt, "a visualization"),
        text_tx(agent, TransactionType::AgentMsg, ""),
    ];
    for (seq, tx) in (1..).zip(entries) {
        append(&store, seq, tx);
    }

    let history = model_history(&store, agent).unwrap();
    let rendered: Vec<(aura_model::Role, &str)> = history
        .iter()
        .map(|m| match m.content.as_slice() {
            [aura_model::ContentBlock::Text { text }] => (m.role, text.as_str()),
            other => panic!("expected one text block, got {other:?}"),
        })
        .collect();
    use aura_model::Role::{Assistant, User};
    assert_eq!(
        rendered,
        [
            (User, "make an app"),
            (Assistant, "Which stack?"),
            (User, "a visualization"),
        ]
    );
    assert!(model_history(&store, AgentId::generate())
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

#[test]
fn recent_record_summaries_load_more_than_a_hundred_entries() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    for seq in 1..=150 {
        append(&store, seq, text_tx(agent, TransactionType::AgentMsg, "hi"));
    }

    let summaries = recent_record_summaries(&store, agent).unwrap();
    let seqs: Vec<u64> = summaries.iter().map(|s| s.seq).collect();
    assert_eq!(seqs, (1..=150).collect::<Vec<u64>>());
}

#[test]
fn load_existing_records_sends_the_history_as_one_command() {
    let dir = tempfile::tempdir().unwrap();
    let store = RocksStore::open(dir.path(), false).unwrap();
    let agent = AgentId::generate();
    for seq in 1..=150 {
        append(&store, seq, text_tx(agent, TransactionType::AgentMsg, "hi"));
    }
    // Smaller than the history, as the app's command channel can be.
    let (commands, mut rx) = mpsc::channel(4);

    load_existing_records(&store, agent, &commands);

    match rx.try_recv() {
        Ok(UiCommand::SetRecords(records)) => assert_eq!(records.len(), 150),
        other => panic!("expected SetRecords, got {other:?}"),
    }
    assert!(rx.try_recv().is_err());
}
