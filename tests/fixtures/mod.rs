//! Shared test fixtures for AURA OS.
//!
//! Provides common test data and helpers used across all crates.

use aura_core::{
    Action, ActionId, ActionKind, AgentId, Decision, Effect, EffectKind, EffectStatus,
    ProposalSet, RecordEntry, ToolCall, ToolResult, Transaction, TransactionType, TxId,
};
use bytes::Bytes;
use std::collections::HashMap;

// ============================================================================
// Agent Fixtures
// ============================================================================

/// Generate a deterministic test agent ID from a seed.
pub fn test_agent_id(seed: u8) -> AgentId {
    AgentId::new([seed; 32])
}

/// Generate a random test agent ID.
pub fn random_agent_id() -> AgentId {
    AgentId::generate()
}

// ============================================================================
// Transaction Fixtures
// ============================================================================

/// Create a simple user prompt transaction.
pub fn user_prompt_tx(agent_id: AgentId, message: &str) -> Transaction {
    Transaction::user_prompt(agent_id, message)
}

/// Create a transaction with specific kind.
pub fn transaction_with_kind(agent_id: AgentId, kind: TransactionType, payload: &str) -> Transaction {
    let payload_bytes = Bytes::from(payload.to_string());
    let tx_id = TxId::from_content(&payload_bytes);
    let ts_ms = 1_700_000_000_000; // Fixed timestamp for determinism
    
    Transaction::new(tx_id, agent_id, ts_ms, kind, payload_bytes)
}

/// Create a session start transaction.
pub fn session_start_tx(agent_id: AgentId) -> Transaction {
    Transaction::session_start(agent_id)
}

/// Create a sequence of user/agent messages for conversation testing.
pub fn conversation_transactions(agent_id: AgentId, exchanges: &[(&str, &str)]) -> Vec<Transaction> {
    exchanges
        .iter()
        .flat_map(|(user_msg, _agent_msg)| {
            vec![
                transaction_with_kind(agent_id, TransactionType::UserPrompt, user_msg),
                // AgentMsg transactions would be created by the system
            ]
        })
        .collect()
}

// ============================================================================
// Action Fixtures
// ============================================================================

/// Create a delegate action for a tool call.
pub fn tool_action(tool: &str, args: serde_json::Value) -> Action {
    let tool_call = ToolCall::new(tool, args);
    Action::delegate_tool(&tool_call).unwrap()
}

/// Create a reason action.
pub fn reason_action(rationale: &str) -> Action {
    Action::new(
        ActionId::generate(),
        ActionKind::Reason,
        Bytes::from(rationale.to_string()),
    )
}

/// Create an action with specific ID for deterministic testing.
pub fn action_with_id(action_id: ActionId, kind: ActionKind, payload: &str) -> Action {
    Action::new(action_id, kind, Bytes::from(payload.to_string()))
}

// ============================================================================
// Effect Fixtures
// ============================================================================

/// Create a committed effect.
pub fn committed_effect(action_id: ActionId, payload: &str) -> Effect {
    Effect::committed_agreement(action_id, Bytes::from(payload.to_string()))
}

/// Create a failed effect.
pub fn failed_effect(action_id: ActionId, error: &str) -> Effect {
    Effect::failed(action_id, EffectKind::Agreement, Bytes::from(error.to_string()))
}

/// Create a pending effect.
pub fn pending_effect(action_id: ActionId) -> Effect {
    Effect::pending(action_id, EffectKind::Agreement)
}

// ============================================================================
// Record Entry Fixtures
// ============================================================================

/// Create a minimal record entry.
pub fn minimal_record_entry(seq: u64, agent_id: AgentId) -> RecordEntry {
    let tx = user_prompt_tx(agent_id, &format!("test message {seq}"));
    RecordEntry::builder(seq, tx)
        .context_hash([seq as u8; 32])
        .proposals(ProposalSet::new())
        .decision(Decision::new())
        .build()
}

/// Create a record entry with tool execution.
pub fn record_entry_with_tool(
    seq: u64,
    agent_id: AgentId,
    tool: &str,
    success: bool,
) -> RecordEntry {
    let tx = user_prompt_tx(agent_id, "execute tool");
    let action = tool_action(tool, serde_json::json!({"path": "."}));
    let action_id = action.action_id;
    
    let effect = if success {
        committed_effect(action_id, "tool output")
    } else {
        failed_effect(action_id, "tool failed")
    };
    
    let mut decision = Decision::new();
    decision.accept(action_id);
    
    RecordEntry::builder(seq, tx)
        .context_hash([seq as u8; 32])
        .proposals(ProposalSet::new())
        .decision(decision)
        .actions(vec![action])
        .effects(vec![effect])
        .build()
}

/// Create a sequence of record entries for history testing.
pub fn record_entry_sequence(agent_id: AgentId, count: usize) -> Vec<RecordEntry> {
    (1..=count)
        .map(|seq| minimal_record_entry(seq as u64, agent_id))
        .collect()
}

/// Create a record sequence with a session boundary.
pub fn record_sequence_with_session_boundary(agent_id: AgentId) -> Vec<RecordEntry> {
    let mut entries = Vec::new();
    
    // Pre-session entries
    entries.push(minimal_record_entry(1, agent_id));
    entries.push(minimal_record_entry(2, agent_id));
    
    // Session start
    let session_tx = session_start_tx(agent_id);
    entries.push(
        RecordEntry::builder(3, session_tx)
            .context_hash([3u8; 32])
            .build(),
    );
    
    // Post-session entries
    entries.push(minimal_record_entry(4, agent_id));
    entries.push(minimal_record_entry(5, agent_id));
    
    entries
}

// ============================================================================
// Tool Call Fixtures
// ============================================================================

/// Create a filesystem list tool call.
pub fn fs_ls_call(path: &str) -> ToolCall {
    ToolCall::fs_ls(path)
}

/// Create a filesystem read tool call.
pub fn fs_read_call(path: &str) -> ToolCall {
    ToolCall::fs_read(path, None)
}

/// Create a filesystem write tool call.
pub fn fs_write_call(path: &str, content: &str) -> ToolCall {
    ToolCall::new(
        "write_file",
        serde_json::json!({
            "path": path,
            "content": content
        }),
    )
}

/// Create a filesystem edit tool call.
pub fn fs_edit_call(path: &str, old_text: &str, new_text: &str) -> ToolCall {
    ToolCall::new(
        "edit_file",
        serde_json::json!({
            "path": path,
            "old_text": old_text,
            "new_text": new_text
        }),
    )
}

/// Create a search code tool call.
pub fn search_code_call(pattern: &str) -> ToolCall {
    ToolCall::new(
        "search_code",
        serde_json::json!({
            "pattern": pattern
        }),
    )
}

/// Create a command run tool call.
pub fn cmd_run_call(program: &str, args: &[&str]) -> ToolCall {
    ToolCall::new(
        "run_command",
        serde_json::json!({
            "program": program,
            "args": args
        }),
    )
}

// ============================================================================
// Tool Result Fixtures
// ============================================================================

/// Create a successful tool result.
pub fn success_tool_result(tool: &str, output: &str) -> ToolResult {
    ToolResult::success(tool, output)
}

/// Create a failed tool result.
pub fn failure_tool_result(tool: &str, error: &str) -> ToolResult {
    ToolResult::failure(tool, error)
}

/// Create a tool result with metadata.
pub fn tool_result_with_metadata(tool: &str, output: &str, metadata: &[(&str, &str)]) -> ToolResult {
    let mut result = ToolResult::success(tool, output);
    for (key, value) in metadata {
        result.metadata.insert((*key).to_string(), (*value).to_string());
    }
    result
}

// ============================================================================
// Test Directory Helpers
// ============================================================================

use std::path::PathBuf;
use tempfile::TempDir;

/// Create a temporary directory with test files.
pub fn temp_dir_with_files(files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().expect("Failed to create temp directory");
    
    for (path, content) in files {
        let file_path = dir.path().join(path);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).expect("Failed to create parent directories");
        }
        std::fs::write(&file_path, content).expect("Failed to write test file");
    }
    
    dir
}

/// Create a temp directory with a typical project structure.
pub fn temp_project_dir() -> TempDir {
    temp_dir_with_files(&[
        ("src/main.rs", "fn main() { println!(\"Hello\"); }"),
        ("src/lib.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }"),
        ("Cargo.toml", "[package]\nname = \"test-project\"\nversion = \"0.1.0\""),
        ("README.md", "# Test Project\n\nA test project."),
        (".gitignore", "target/\n*.log"),
    ])
}

/// Get the path to a file in the temp directory.
pub fn temp_file_path(dir: &TempDir, relative: &str) -> PathBuf {
    dir.path().join(relative)
}

// ============================================================================
// Assertion Helpers
// ============================================================================

/// Assert that a ToolResult is successful.
#[track_caller]
pub fn assert_tool_success(result: &ToolResult) {
    assert!(result.ok, "Expected tool success, got failure: {:?}", 
            String::from_utf8_lossy(&result.stderr));
}

/// Assert that a ToolResult failed.
#[track_caller]
pub fn assert_tool_failure(result: &ToolResult) {
    assert!(!result.ok, "Expected tool failure, got success: {:?}",
            String::from_utf8_lossy(&result.stdout));
}

/// Assert that an Effect is committed.
#[track_caller]
pub fn assert_effect_committed(effect: &Effect) {
    assert_eq!(effect.status, EffectStatus::Committed,
               "Expected committed effect, got {:?}", effect.status);
}

/// Assert that an Effect failed.
#[track_caller]
pub fn assert_effect_failed(effect: &Effect) {
    assert_eq!(effect.status, EffectStatus::Failed,
               "Expected failed effect, got {:?}", effect.status);
}

// ============================================================================
// JSON Fixtures
// ============================================================================

/// Common JSON schemas for testing.
pub mod json {
    use serde_json::json;
    
    /// A simple object schema.
    pub fn object_schema() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "value": { "type": "integer" }
            },
            "required": ["name"]
        })
    }
    
    /// The fs_read tool input schema.
    pub fn fs_read_schema() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to read" },
                "max_bytes": { "type": "integer", "description": "Max bytes to read" }
            },
            "required": ["path"]
        })
    }
    
    /// The fs_write tool input schema.
    pub fn fs_write_schema() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" },
                "create_dirs": { "type": "boolean" }
            },
            "required": ["path", "content"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_fixtures_compile() {
        let agent = test_agent_id(1);
        let tx = user_prompt_tx(agent, "test");
        let entry = minimal_record_entry(1, agent);
        
        assert_eq!(entry.seq, 1);
        assert_eq!(tx.agent_id, agent);
    }
    
    #[test]
    fn test_temp_dir_with_files() {
        let dir = temp_dir_with_files(&[
            ("test.txt", "hello"),
            ("nested/file.txt", "world"),
        ]);
        
        assert!(dir.path().join("test.txt").exists());
        assert!(dir.path().join("nested/file.txt").exists());
    }
}
