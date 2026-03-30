//! Tests for turn processor.

use super::*;
use aura_reasoner::{MockProvider, MockResponse};
use aura_store::RocksStore;
use aura_tools::DefaultToolRegistry;
use std::collections::HashMap;
use tempfile::TempDir;

fn create_test_processor() -> (
    TurnProcessor<MockProvider, RocksStore, DefaultToolRegistry>,
    TempDir,
    TempDir,
) {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();

    let provider = Arc::new(MockProvider::simple_response("Hello!"));
    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let executor = ExecutorRouter::new();
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);
    (processor, db_dir, ws_dir)
}

#[tokio::test]
async fn test_simple_turn() {
    let (processor, _db_dir, _ws_dir) = create_test_processor();

    let tx = Transaction::user_prompt(AgentId::generate(), "Hello");
    let result = processor.process_turn(tx.agent_id, tx, 1).await.unwrap();

    assert_eq!(result.steps, 1);
    assert!(!result.had_failures);
    assert!(result.final_message.is_some());
}

#[tokio::test]
async fn test_turn_with_tool_use() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();

    let provider = Arc::new(
        MockProvider::new()
            .with_response(MockResponse::tool_use(
                "tool_1",
                "fs.ls",
                serde_json::json!({ "path": "." }),
            ))
            .with_response(MockResponse::text("I listed the files.")),
    );

    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let executor = ExecutorRouter::new();
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

    let tx = Transaction::user_prompt(AgentId::generate(), "List files");
    let result = processor.process_turn(tx.agent_id, tx, 1).await.unwrap();

    assert_eq!(result.steps, 2);
}

#[tokio::test]
async fn test_max_steps_limit() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();

    let provider = Arc::new(
        MockProvider::new().with_default_response(MockResponse::tool_use(
            "tool_1",
            "fs.ls",
            serde_json::json!({ "path": "." }),
        )),
    );

    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let executor = ExecutorRouter::new();
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        max_steps: 3,
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

    let tx = Transaction::user_prompt(AgentId::generate(), "Keep using tools");
    let result = processor.process_turn(tx.agent_id, tx, 1).await.unwrap();

    assert_eq!(result.steps, 3);
}

#[tokio::test]
async fn test_process_step_returns_end_turn() {
    let (processor, _db_dir, _ws_dir) = create_test_processor();

    let messages = vec![Message::user("Hello")];
    let agent_id = AgentId::generate();
    let mut tool_cache: ToolCache = HashMap::new();

    let result = processor
        .process_step(&messages, agent_id, &mut tool_cache, &StepConfig::default())
        .await
        .unwrap();

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert!(result.executed_tools.is_empty());
    assert!(!result.had_failures);
}

#[tokio::test]
async fn test_process_step_returns_tool_use() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();

    let provider = Arc::new(MockProvider::new().with_response(MockResponse::tool_use(
        "tool_1",
        "list_files",
        serde_json::json!({ "path": "." }),
    )));

    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let executor = ExecutorRouter::new();
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

    let messages = vec![Message::user("List files")];
    let agent_id = AgentId::generate();
    let mut tool_cache: ToolCache = HashMap::new();

    let result = processor
        .process_step(&messages, agent_id, &mut tool_cache, &StepConfig::default())
        .await
        .unwrap();

    assert_eq!(result.stop_reason, StopReason::ToolUse);
    assert!(!result.executed_tools.is_empty());
}

#[tokio::test]
async fn test_process_step_respects_model_override() {
    let (processor, _db_dir, _ws_dir) = create_test_processor();

    let messages = vec![Message::user("Hello")];
    let agent_id = AgentId::generate();
    let mut tool_cache: ToolCache = HashMap::new();

    let step_config = StepConfig {
        model_override: Some("override-model".to_string()),
        ..StepConfig::default()
    };

    let result = processor
        .process_step(&messages, agent_id, &mut tool_cache, &step_config)
        .await
        .unwrap();

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert!(!result.had_failures);
}

#[tokio::test]
async fn test_run_turn_loop_backward_compat() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();

    let provider = Arc::new(
        MockProvider::new()
            .with_response(MockResponse::text("Hello from store!"))
            .with_response(MockResponse::text("Hello from messages!")),
    );

    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let executor = ExecutorRouter::new();
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

    let agent_id = AgentId::generate();
    let tx = Transaction::user_prompt(agent_id, "Hello");
    let result_store = processor.process_turn(tx.agent_id, tx, 1).await.unwrap();

    let messages = vec![Message::user("Hello".to_string())];
    let result_msgs = processor
        .process_turn_with_messages(agent_id, messages)
        .await
        .unwrap();

    assert_eq!(result_store.steps, result_msgs.steps);
    assert_eq!(result_store.steps, 1);
    assert!(!result_store.had_failures);
    assert!(!result_msgs.had_failures);
}

#[test]
fn test_step_config_default() {
    let config = StepConfig::default();
    assert!(config.thinking_budget.is_none());
    assert!(config.model_override.is_none());
    assert!(config.max_tool_calls.is_none());
}

#[tokio::test]
async fn test_multiple_sequential_tool_calls() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();

    let provider = Arc::new(
        MockProvider::new()
            .with_response(MockResponse::tool_use(
                "tool_1",
                "list_files",
                serde_json::json!({ "path": "." }),
            ))
            .with_response(MockResponse::tool_use(
                "tool_2",
                "read_file",
                serde_json::json!({ "path": "file.txt" }),
            ))
            .with_response(MockResponse::text("All done.")),
    );

    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let executor = ExecutorRouter::new();
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

    let tx = Transaction::user_prompt(AgentId::generate(), "Read files");
    let result = processor.process_turn(tx.agent_id, tx, 1).await.unwrap();

    assert_eq!(result.steps, 3);
    assert!(result.final_message.is_some());
}

#[tokio::test]
async fn test_max_steps_budget_enforcement() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();

    let provider = Arc::new(
        MockProvider::new().with_default_response(MockResponse::tool_use(
            "tool_loop",
            "list_files",
            serde_json::json!({ "path": "." }),
        )),
    );

    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let executor = ExecutorRouter::new();
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        max_steps: 2,
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

    let tx = Transaction::user_prompt(AgentId::generate(), "Loop forever");
    let result = processor.process_turn(tx.agent_id, tx, 1).await.unwrap();

    assert_eq!(result.steps, 2);
}

#[tokio::test]
async fn test_cancellation_stops_turn() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();

    let provider = Arc::new(
        MockProvider::new()
            .with_default_response(MockResponse::tool_use(
                "tool_1",
                "list_files",
                serde_json::json!({ "path": "." }),
            ))
            .with_latency(50),
    );

    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let executor = ExecutorRouter::new();
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        max_steps: 100,
        ..TurnConfig::default()
    };

    let token = CancellationToken::new();
    let mut processor = TurnProcessor::new(provider, store, executor, tool_registry, config);
    processor.set_cancellation_token(token.clone());

    // Cancel immediately
    token.cancel();

    let tx = Transaction::user_prompt(AgentId::generate(), "Do work");
    let result = processor.process_turn(tx.agent_id, tx, 1).await.unwrap();

    assert!(result.cancelled);
    assert_eq!(result.steps, 0);
}

#[tokio::test]
async fn test_process_turn_with_messages_entry_point() {
    let (processor, _db_dir, _ws_dir) = create_test_processor();

    let messages = vec![Message::user("Hello via messages API")];
    let agent_id = AgentId::generate();

    let result = processor
        .process_turn_with_messages(agent_id, messages)
        .await
        .unwrap();

    assert_eq!(result.steps, 1);
    assert!(!result.had_failures);
    assert!(result.final_message.is_some());
}

#[tokio::test]
async fn test_turn_result_token_accounting() {
    let (processor, _db_dir, _ws_dir) = create_test_processor();

    let tx = Transaction::user_prompt(AgentId::generate(), "Hello");
    let result = processor.process_turn(tx.agent_id, tx, 1).await.unwrap();

    assert!(result.total_input_tokens > 0 || result.total_output_tokens > 0);
}

#[tokio::test]
async fn test_replay_mode_skips_model() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();

    let provider = Arc::new(MockProvider::new().with_failure());
    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let executor = ExecutorRouter::new();
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    let config = TurnConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        replay_mode: true,
        ..TurnConfig::default()
    };

    let processor = TurnProcessor::new(provider, store, executor, tool_registry, config);

    let tx = Transaction::user_prompt(AgentId::generate(), "Test replay");
    let result = processor.process_turn(tx.agent_id, tx, 1).await.unwrap();

    assert_eq!(result.steps, 1);
    assert!(!result.had_failures);
}
