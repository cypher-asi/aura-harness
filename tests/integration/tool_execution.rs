//! Tool execution integration tests.
//!
//! Tests the full tool execution pipeline from action to effect.

use aura_core::{Action, ActionId, AgentId, ToolCall};
use aura_kernel::ExecuteContext;
use aura_kernel::ExecutorRouter;
use aura_tools::{Sandbox, ToolConfig, ToolExecutor};
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

/// Create an executor router configured for command execution tests.
///
/// Phase 2 hardening made `binary_allowlist` fail closed, so command
/// tests must supply a non-empty list. `echo`, `dir`, `ls`, `cmd` cover
/// the shapes used below.
fn create_cmd_executor() -> ExecutorRouter {
    let config = ToolConfig {
        binary_allowlist: vec![
            "echo".to_string(),
            "dir".to_string(),
            "ls".to_string(),
            "cmd".to_string(),
            "sh".to_string(),
        ],
        ..ToolConfig::default()
    };
    let mut router = ExecutorRouter::new();
    router.add_executor(Arc::new(ToolExecutor::new(config)));
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
    let action = Action::delegate_tool(&tool_call).unwrap();

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
    let action = Action::delegate_tool(&tool_call).unwrap();

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
        "write_file",
        serde_json::json!({
            "path": "new_file.txt",
            "content": "New content here!"
        }),
    );
    let action = Action::delegate_tool(&tool_call).unwrap();

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
        "edit_file",
        serde_json::json!({
            "path": "hello.txt",
            "old_text": "World",
            "new_text": "AURA"
        }),
    );
    let action = Action::delegate_tool(&tool_call).unwrap();

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
    let action = Action::delegate_tool(&tool_call).unwrap();

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
    let action = Action::delegate_tool(&tool_call).unwrap();

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
    let action = Action::delegate_tool(&tool_call).unwrap();

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
    let action = Action::delegate_tool(&tool_call).unwrap();

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
    let action = Action::delegate_tool(&tool_call).unwrap();

    let effect = executor.execute(&ctx, &action).await;

    assert_eq!(effect.status, aura_core::EffectStatus::Failed);
}

#[tokio::test]
async fn test_unknown_tool() {
    let workspace = create_test_workspace();
    let executor = create_executor();
    let ctx = create_context(&workspace);

    let tool_call = ToolCall::new("unknown_tool", serde_json::json!({}));
    let action = Action::delegate_tool(&tool_call).unwrap();

    let effect = executor.execute(&ctx, &action).await;

    assert_eq!(effect.status, aura_core::EffectStatus::Failed);
}

#[tokio::test]
async fn test_missing_required_argument() {
    let workspace = create_test_workspace();
    let executor = create_executor();
    let ctx = create_context(&workspace);

    // fs_read requires 'path' argument
    let tool_call = ToolCall::new("read_file", serde_json::json!({}));
    let action = Action::delegate_tool(&tool_call).unwrap();

    let effect = executor.execute(&ctx, &action).await;

    assert_eq!(effect.status, aura_core::EffectStatus::Failed);
}

// ============================================================================
// Command Execution Tests
// ============================================================================

#[tokio::test]
async fn test_cmd_run_simple() {
    let workspace = create_test_workspace();
    let executor = create_cmd_executor();
    let ctx = create_context(&workspace);

    // Test a simple echo command via the direct (no-shell) path.
    let tool_call = ToolCall::new(
        "run_command",
        serde_json::json!({
            "program": "echo",
            "args": ["hello"]
        }),
    );
    let action = Action::delegate_tool(&tool_call).unwrap();

    let effect = executor.execute(&ctx, &action).await;

    // `echo.exe` may be absent on a bare Windows host; both success and
    // a failed-but-non-error effect are acceptable. Hard-fail only on
    // Unix where `echo` is always available.
    if cfg!(windows) {
        assert!(matches!(
            effect.status,
            aura_core::EffectStatus::Committed | aura_core::EffectStatus::Failed
        ));
    } else {
        assert_eq!(effect.status, aura_core::EffectStatus::Committed);
    }
}

#[tokio::test]
async fn test_cmd_run_in_workspace_dir() {
    let workspace = create_test_workspace();
    let executor = create_cmd_executor();
    let ctx = create_context(&workspace);

    // Phase 2: `dir` is a cmd.exe builtin and is no longer reachable
    // through the no-shell path. Route via `cmd /c dir` / `ls` using
    // explicit `program` + `args`.
    #[cfg(windows)]
    let tool_call = ToolCall::new(
        "run_command",
        serde_json::json!({
            "program": "cmd",
            "args": ["/c", "dir"]
        }),
    );
    #[cfg(not(windows))]
    let tool_call = ToolCall::new(
        "run_command",
        serde_json::json!({
            "program": "ls",
            "args": ["-1"]
        }),
    );

    let action = Action::delegate_tool(&tool_call).unwrap();
    let effect = executor.execute(&ctx, &action).await;

    assert_eq!(effect.status, aura_core::EffectStatus::Committed);
}
