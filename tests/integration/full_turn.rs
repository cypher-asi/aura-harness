//! Full turn integration tests.
//!
//! Tests complete turns from user prompt through model response to tool execution.

use aura_core::{AgentId, Transaction};
use aura_executor::ExecutorRouter;
use aura_kernel::{TurnConfig, TurnProcessor};
use aura_reasoner::{MockProvider, MockResponse};
use aura_store::RocksStore;
use aura_tools::{DefaultToolRegistry, ToolExecutor};
use std::sync::Arc;
use tempfile::TempDir;

/// Create a test environment with all components.
fn create_test_env() -> (
    TurnProcessor<MockProvider, RocksStore, DefaultToolRegistry>,
    AgentId,
    TempDir,
    TempDir,
) {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();

    let provider = Arc::new(MockProvider::simple_response("Hello from AURA!"));
    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    
    // Create executor router with tool executor
    let mut executor = ExecutorRouter::new();
    executor.add_executor(Arc::new(ToolExecutor::with_defaults()));
    
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        max_steps: 10,
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

    (processor, agent_id, db_dir, ws_dir)
}

#[tokio::test]
async fn test_simple_turn_no_tools() {
    let (processor, agent_id, _db_dir, _ws_dir) = create_test_env();

    let tx = Transaction::user_prompt(agent_id, "Hello, AURA!");
    let result = processor.process_turn(agent_id, tx, 1).await.unwrap();

    assert_eq!(result.steps, 1);
    assert!(!result.had_failures);
    assert!(result.final_message.is_some());
    
    let message = result.final_message.unwrap();
    assert_eq!(message.text_content(), "Hello from AURA!");
}

#[tokio::test]
async fn test_turn_with_tool_use() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();

    // Create a test file in the workspace
    let test_file = ws_dir.path().join(agent_id.to_hex()).join("test.txt");
    std::fs::create_dir_all(test_file.parent().unwrap()).unwrap();
    std::fs::write(&test_file, "Hello from file!").unwrap();

    // Provider that requests a tool then responds
    let provider = Arc::new(
        MockProvider::new()
            .with_response(MockResponse::tool_use(
                "tool_1",
                "fs_read",
                serde_json::json!({ "path": "test.txt" }),
            ))
            .with_response(MockResponse::text("I read the file!")),
    );

    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let mut executor = ExecutorRouter::new();
    executor.add_executor(Arc::new(ToolExecutor::with_defaults()));
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

    let tx = Transaction::user_prompt(agent_id, "Read test.txt");
    let result = processor.process_turn(agent_id, tx, 1).await.unwrap();

    // Should have 2 steps: tool use + final response
    assert_eq!(result.steps, 2);
    
    // First step should have executed tool
    assert!(!result.entries[0].executed_tools.is_empty());
    assert_eq!(result.entries[0].executed_tools[0].tool_name, "fs_read");
}

#[tokio::test]
async fn test_turn_tool_denied_by_policy() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();

    // Provider that requests an unknown tool
    let provider = Arc::new(
        MockProvider::new()
            .with_response(MockResponse::tool_use(
                "tool_1",
                "unknown_dangerous_tool",
                serde_json::json!({}),
            ))
            .with_response(MockResponse::text("OK, that didn't work.")),
    );

    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let mut executor = ExecutorRouter::new();
    executor.add_executor(Arc::new(ToolExecutor::with_defaults()));
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

    let tx = Transaction::user_prompt(agent_id, "Use dangerous tool");
    let result = processor.process_turn(agent_id, tx, 1).await.unwrap();

    // Tool should have been denied
    assert!(result.entries[0].executed_tools[0].is_error);
}

#[tokio::test]
async fn test_turn_max_steps_limit() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();

    // Provider that always requests tools (never ends)
    let provider = Arc::new(
        MockProvider::new().with_default_response(MockResponse::tool_use(
            "tool_1",
            "fs_ls",
            serde_json::json!({ "path": "." }),
        )),
    );

    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    
    // Create workspace for agent
    let agent_workspace = ws_dir.path().join(agent_id.to_hex());
    std::fs::create_dir_all(&agent_workspace).unwrap();
    
    let mut executor = ExecutorRouter::new();
    executor.add_executor(Arc::new(ToolExecutor::with_defaults()));
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        max_steps: 3, // Limit to 3 steps
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

    let tx = Transaction::user_prompt(agent_id, "Keep running tools");
    let result = processor.process_turn(agent_id, tx, 1).await.unwrap();

    // Should stop at max_steps
    assert_eq!(result.steps, 3);
}

#[tokio::test]
async fn test_turn_record_entry_creation() {
    let (processor, agent_id, _db_dir, _ws_dir) = create_test_env();

    let tx = Transaction::user_prompt(agent_id, "Test");
    let result = processor.process_turn(agent_id, tx.clone(), 1).await.unwrap();

    // Create a record entry from the result
    let entry = processor.to_record_entry(1, tx, &result, [0u8; 32]).unwrap();

    assert_eq!(entry.seq, 1);
    assert_eq!(entry.tx.agent_id, agent_id);
}
