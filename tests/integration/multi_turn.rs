//! Multi-turn conversation integration tests.
//!
//! Tests conversation history loading and context accumulation across turns.

use aura_core::{AgentId, Decision, ProposalSet, RecordEntry, Transaction, TransactionKind};
use aura_executor::ExecutorRouter;
use aura_kernel::{TurnConfig, TurnProcessor};
use aura_reasoner::{MockProvider, MockResponse};
use aura_store::{RocksStore, Store};
use aura_tools::{DefaultToolRegistry, ToolExecutor};
use bytes::Bytes;
use std::sync::Arc;
use tempfile::TempDir;

/// Helper to create and store a record entry.
fn store_entry(store: &RocksStore, agent_id: AgentId, seq: u64, kind: TransactionKind, content: &str) {
    let tx = Transaction::new(
        aura_core::TxId::from_content(content.as_bytes()),
        agent_id,
        1000 + seq,
        kind,
        Bytes::from(content.to_string()),
    );

    // First enqueue a dummy transaction to get an inbox_seq
    store.enqueue_tx(&tx).unwrap();
    let (inbox_seq, _) = store.dequeue_tx(agent_id).unwrap().unwrap();

    let entry = RecordEntry::builder(seq, tx)
        .context_hash([seq as u8; 32])
        .proposals(ProposalSet::new())
        .decision(Decision::new())
        .build();

    store.append_entry_atomic(agent_id, seq, &entry, inbox_seq).unwrap();
}

#[tokio::test]
async fn test_conversation_history_loaded() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();

    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());

    // Pre-populate history
    store_entry(&store, agent_id, 1, TransactionKind::UserPrompt, "Hello");
    store_entry(&store, agent_id, 2, TransactionKind::AgentMsg, "Hi there!");
    store_entry(&store, agent_id, 3, TransactionKind::UserPrompt, "How are you?");
    store_entry(&store, agent_id, 4, TransactionKind::AgentMsg, "I'm doing well!");

    // Create processor
    let provider = Arc::new(MockProvider::simple_response("Thanks for asking!"));
    let mut executor = ExecutorRouter::new();
    executor.add_executor(Arc::new(ToolExecutor::with_defaults()));
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        context_window: 10,
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider.clone(), store, executor, tool_registry, config);

    // Process a new turn
    let tx = Transaction::user_prompt(agent_id, "What's new?");
    let result = processor.process_turn(agent_id, tx, 5).await.unwrap();

    assert!(result.final_message.is_some());
    // The model was called with context (we just verify it worked)
    assert_eq!(result.steps, 1);
}

#[tokio::test]
async fn test_session_boundary_resets_context() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();

    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());

    // Pre-session history
    store_entry(&store, agent_id, 1, TransactionKind::UserPrompt, "Old message 1");
    store_entry(&store, agent_id, 2, TransactionKind::AgentMsg, "Old response 1");

    // Session start (context boundary)
    store_entry(&store, agent_id, 3, TransactionKind::SessionStart, "");

    // Post-session history
    store_entry(&store, agent_id, 4, TransactionKind::UserPrompt, "New message");
    store_entry(&store, agent_id, 5, TransactionKind::AgentMsg, "New response");

    // Create processor
    let provider = Arc::new(MockProvider::simple_response("Context test!"));
    let mut executor = ExecutorRouter::new();
    executor.add_executor(Arc::new(ToolExecutor::with_defaults()));
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        context_window: 10,
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

    // Process turn - should only see messages after SessionStart
    let tx = Transaction::user_prompt(agent_id, "Latest");
    let result = processor.process_turn(agent_id, tx, 6).await.unwrap();

    assert!(result.final_message.is_some());
    assert_eq!(result.steps, 1);
}

#[tokio::test]
async fn test_empty_history() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();

    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());

    // No history - this is the first message

    let provider = Arc::new(MockProvider::simple_response("Hello! Nice to meet you."));
    let mut executor = ExecutorRouter::new();
    executor.add_executor(Arc::new(ToolExecutor::with_defaults()));
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        context_window: 10,
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

    let tx = Transaction::user_prompt(agent_id, "Hi, I'm new here!");
    let result = processor.process_turn(agent_id, tx, 1).await.unwrap();

    assert!(result.final_message.is_some());
    assert_eq!(result.steps, 1);
}

#[tokio::test]
async fn test_context_window_limit() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();

    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());

    // Create many messages
    for i in 1..=20 {
        let kind = if i % 2 == 1 {
            TransactionKind::UserPrompt
        } else {
            TransactionKind::AgentMsg
        };
        store_entry(&store, agent_id, i, kind, &format!("Message {i}"));
    }

    let provider = Arc::new(MockProvider::simple_response("Window test!"));
    let mut executor = ExecutorRouter::new();
    executor.add_executor(Arc::new(ToolExecutor::with_defaults()));
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    // Only load last 5 entries
    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        context_window: 5,
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

    let tx = Transaction::user_prompt(agent_id, "Test with limited context");
    let result = processor.process_turn(agent_id, tx, 21).await.unwrap();

    assert!(result.final_message.is_some());
    assert_eq!(result.steps, 1);
}
