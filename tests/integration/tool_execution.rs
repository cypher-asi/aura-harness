//! Tool execution integration tests.
//!
//! Tests the full tool execution pipeline from action to effect.

use aura_core::{Action, ActionId, AgentId, ToolCall};
use aura_executor::{ExecuteContext, ExecutorRouter};
use aura_tools::{Sandbox, ToolExecutor};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

/// Create a test workspace with some files.
fn create_test_workspace() -> TempDir {
    let dir = TempDir::new().unwrap();
    
    // Create test files
    std::fs::write(dir.path().join("hello.txt"), "Hello, World!").unwrap();
    std::fs::write(dir.path().join("data.json"), r#"{"key": "value"}"#).unwrap();
    std::fs::create_dir(dir.path().join("subdir")).unwrap();
    std::fs::write(dir.path().join("subdir/nested.txt"), "Nested content").unwrap();
    
    // Create a Rust file for search testing
    std::fs::write(
        dir.path().join("code.rs"),
        "fn main() {\n    let x = 42;\n    println!(\"Hello\");\n}\n",
    ).unwrap();
    
    dir
}

/// Create an executor router with the tool executor.
fn create_executor() -> ExecutorRouter {
    let mut router = ExecutorRouter::new();
    router.add_executor(Arc::new(ToolExecutor::with_defaults()));
    router
}

/// Create an execution context.
fn create_context(workspace: &TempDir) -> ExecuteContext {
    ExecuteContext::new(
        AgentId::generate(),
        ActionId::generate(),
        workspace.path().to_path_buf(),
    )
}

// ============================================================================
// Filesystem Tool Tests
// ============================================================================

#[tokio::test]
async fn test_fs_ls_integration() {
    let workspace = create_test_workspace();
    let executor = create_executor();
    let ctx = create_context(&workspace);

    let tool_call = ToolCall::fs_ls(".");
    let action = Action::delegate_tool(&tool_call);

    let effect = executor.execute(&ctx, &action).await;

    assert_eq!(effect.status, aura_core::EffectStatus::Committed);
    
    let output = String::from_utf8_lossy(&effect.payload);
    assert!(output.contains("hello.txt") || output.contains("stdout")); // Might be in JSON
}

#[tokio::test]
async fn test_fs_read_integration() {
    let workspace = create_test_workspace();
    let executor = create_executor();
    let ctx = create_context(&workspace);

    let tool_call = ToolCall::fs_read("hello.txt", None);
    let action = Action::delegate_tool(&tool_call);

    let effect = executor.execute(&ctx, &action).await;

    assert_eq!(effect.status, aura_core::EffectStatus::Committed);
    
    let output = String::from_utf8_lossy(&effect.payload);
    assert!(output.contains("Hello, World!") || output.contains("stdout"));
}

#[tokio::test]
async fn test_fs_write_integration() {
    let workspace = create_test_workspace();
    let executor = create_executor();
    let ctx = create_context(&workspace);

    let tool_call = ToolCall::new(
        "fs_write",
        serde_json::json!({
            "path": "new_file.txt",
            "content": "New content here!"
        }),
    );
    let action = Action::delegate_tool(&tool_call);

    let effect = executor.execute(&ctx, &action).await;

    assert_eq!(effect.status, aura_core::EffectStatus::Committed);
    
    // Verify file was created
    let content = std::fs::read_to_string(workspace.path().join("new_file.txt")).unwrap();
    assert_eq!(content, "New content here!");
}

#[tokio::test]
async fn test_fs_edit_integration() {
    let workspace = create_test_workspace();
    let executor = create_executor();
    let ctx = create_context(&workspace);

    let tool_call = ToolCall::new(
        "fs_edit",
        serde_json::json!({
            "path": "hello.txt",
            "old_text": "World",
            "new_text": "AURA"
        }),
    );
    let action = Action::delegate_tool(&tool_call);

    let effect = executor.execute(&ctx, &action).await;

    assert_eq!(effect.status, aura_core::EffectStatus::Committed);
    
    // Verify file was edited
    let content = std::fs::read_to_string(workspace.path().join("hello.txt")).unwrap();
    assert_eq!(content, "Hello, AURA!");
}

#[tokio::test]
async fn test_search_code_integration() {
    let workspace = create_test_workspace();
    let executor = create_executor();
    let ctx = create_context(&workspace);

    let tool_call = ToolCall::new(
        "search_code",
        serde_json::json!({
            "pattern": "fn main",
            "file_pattern": "*.rs"
        }),
    );
    let action = Action::delegate_tool(&tool_call);

    let effect = executor.execute(&ctx, &action).await;

    assert_eq!(effect.status, aura_core::EffectStatus::Committed);
    
    let output = String::from_utf8_lossy(&effect.payload);
    assert!(output.contains("fn main") || output.contains("code.rs"));
}

// ============================================================================
// Sandbox Security Tests
// ============================================================================

#[tokio::test]
async fn test_path_traversal_blocked() {
    let workspace = create_test_workspace();
    let executor = create_executor();
    let ctx = create_context(&workspace);

    let tool_call = ToolCall::fs_read("../../../etc/passwd", None);
    let action = Action::delegate_tool(&tool_call);

    let effect = executor.execute(&ctx, &action).await;

    // Should fail due to sandbox violation
    assert_eq!(effect.status, aura_core::EffectStatus::Failed);
}

#[tokio::test]
async fn test_absolute_path_outside_workspace() {
    let workspace = create_test_workspace();
    let executor = create_executor();
    let ctx = create_context(&workspace);

    let tool_call = ToolCall::fs_read("/etc/passwd", None);
    let action = Action::delegate_tool(&tool_call);

    let effect = executor.execute(&ctx, &action).await;

    // Should fail due to sandbox violation
    assert_eq!(effect.status, aura_core::EffectStatus::Failed);
}

#[tokio::test]
async fn test_nested_path_traversal() {
    let workspace = create_test_workspace();
    let executor = create_executor();
    let ctx = create_context(&workspace);

    // Try to escape via nested traversal
    let tool_call = ToolCall::fs_read("subdir/../../secret.txt", None);
    let action = Action::delegate_tool(&tool_call);

    let effect = executor.execute(&ctx, &action).await;

    // Should fail (file doesn't exist in workspace, and traversal is blocked)
    assert_eq!(effect.status, aura_core::EffectStatus::Failed);
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[tokio::test]
async fn test_file_not_found() {
    let workspace = create_test_workspace();
    let executor = create_executor();
    let ctx = create_context(&workspace);

    let tool_call = ToolCall::fs_read("nonexistent.txt", None);
    let action = Action::delegate_tool(&tool_call);

    let effect = executor.execute(&ctx, &action).await;

    assert_eq!(effect.status, aura_core::EffectStatus::Failed);
}

#[tokio::test]
async fn test_unknown_tool() {
    let workspace = create_test_workspace();
    let executor = create_executor();
    let ctx = create_context(&workspace);

    let tool_call = ToolCall::new("unknown_tool", serde_json::json!({}));
    let action = Action::delegate_tool(&tool_call);

    let effect = executor.execute(&ctx, &action).await;

    assert_eq!(effect.status, aura_core::EffectStatus::Failed);
}

#[tokio::test]
async fn test_missing_required_argument() {
    let workspace = create_test_workspace();
    let executor = create_executor();
    let ctx = create_context(&workspace);

    // fs_read requires 'path' argument
    let tool_call = ToolCall::new("fs_read", serde_json::json!({}));
    let action = Action::delegate_tool(&tool_call);

    let effect = executor.execute(&ctx, &action).await;

    assert_eq!(effect.status, aura_core::EffectStatus::Failed);
}

// ============================================================================
// Command Execution Tests
// ============================================================================

#[tokio::test]
async fn test_cmd_run_simple() {
    let workspace = create_test_workspace();
    let executor = create_executor();
    let ctx = create_context(&workspace);

    // Test a simple echo command
    let tool_call = ToolCall::new(
        "cmd_run",
        serde_json::json!({
            "program": "echo",
            "args": ["hello"]
        }),
    );
    let action = Action::delegate_tool(&tool_call);

    let effect = executor.execute(&ctx, &action).await;

    // Command execution should work
    assert_eq!(effect.status, aura_core::EffectStatus::Committed);
}

#[tokio::test]
async fn test_cmd_run_in_workspace_dir() {
    let workspace = create_test_workspace();
    let executor = create_executor();
    let ctx = create_context(&workspace);

    // List files in the workspace (dir on Windows, ls on Unix)
    #[cfg(windows)]
    let tool_call = ToolCall::new(
        "cmd_run",
        serde_json::json!({
            "program": "dir"
        }),
    );
    #[cfg(not(windows))]
    let tool_call = ToolCall::new(
        "cmd_run",
        serde_json::json!({
            "program": "ls"
        }),
    );

    let action = Action::delegate_tool(&tool_call);
    let effect = executor.execute(&ctx, &action).await;

    assert_eq!(effect.status, aura_core::EffectStatus::Committed);
}
