use super::*;

fn make_tool(name: &str, input: serde_json::Value) -> ToolCallInfo {
    ToolCallInfo {
        id: "test_id".to_string(),
        name: name.to_string(),
        input,
    }
}

#[test]
fn test_detect_blocked_writes_allows_first_write() {
    let ctx = BlockingContext::new(12);
    let tool = make_tool("write_file", serde_json::json!({"path": "test.rs"}));
    let result = detect_blocked_writes(&tool, &ctx).unwrap();
    assert!(!result.blocked);
}

#[test]
fn test_detect_blocked_writes_blocks_second_write() {
    let mut ctx = BlockingContext::new(12);
    ctx.written_paths.insert("test.rs".to_string());
    let tool = make_tool("write_file", serde_json::json!({"path": "test.rs"}));
    let result = detect_blocked_writes(&tool, &ctx).unwrap();
    assert!(result.blocked);
    assert!(result.recovery_message.unwrap().contains("already wrote"));
}

#[test]
fn test_detect_blocked_writes_allows_edit_file_on_written_path() {
    let mut ctx = BlockingContext::new(12);
    ctx.written_paths.insert("test.rs".to_string());
    let tool = make_tool("edit_file", serde_json::json!({"path": "test.rs"}));
    let result = detect_blocked_writes(&tool, &ctx);
    assert!(
        result.is_none(),
        "edit_file should bypass the duplicate-write detector"
    );
}

#[test]
fn test_detect_blocked_writes_allows_delete_file_on_written_path() {
    let mut ctx = BlockingContext::new(12);
    ctx.written_paths.insert("test.rs".to_string());
    let tool = make_tool("delete_file", serde_json::json!({"path": "test.rs"}));
    let result = detect_blocked_writes(&tool, &ctx);
    assert!(
        result.is_none(),
        "delete_file should bypass the duplicate-write detector"
    );
}

#[test]
fn test_detect_blocked_write_failures_at_threshold() {
    let mut ctx = BlockingContext::new(12);
    ctx.write_failures.insert("test.rs".to_string(), 3);
    let tool = make_tool("write_file", serde_json::json!({"path": "test.rs"}));
    let result = detect_blocked_write_failures(&tool, &ctx).unwrap();
    assert!(result.blocked);
}

#[test]
fn test_detect_blocked_commands_under_threshold() {
    let mut ctx = BlockingContext::new(12);
    ctx.consecutive_cmd_failures = 4;
    let tool = make_tool("run_command", serde_json::json!({"command": "cargo build"}));
    let result = detect_blocked_commands(&tool, &ctx).unwrap();
    assert!(!result.blocked);
}

#[test]
fn test_detect_blocked_commands_at_threshold() {
    let mut ctx = BlockingContext::new(12);
    ctx.consecutive_cmd_failures = 5;
    let tool = make_tool("run_command", serde_json::json!({"command": "cargo build"}));
    let result = detect_blocked_commands(&tool, &ctx).unwrap();
    assert!(result.blocked);
}

#[test]
fn test_detect_blocked_exploration_allows_under() {
    let ctx = BlockingContext::new(12);
    let tool = make_tool("read_file", serde_json::json!({"path": "test.rs"}));
    let result = detect_blocked_exploration(&tool, &ctx).unwrap();
    assert!(!result.blocked);
}

#[test]
fn test_detect_blocked_exploration_when_exceeded() {
    let mut ctx = BlockingContext::new(12);
    ctx.exploration_count = 12;
    let tool = make_tool("read_file", serde_json::json!({"path": "test.rs"}));
    let result = detect_blocked_exploration(&tool, &ctx).unwrap();
    assert!(result.blocked);
}

#[test]
fn test_decrement_cooldowns_reduces_and_removes() {
    let mut ctx = BlockingContext::new(12);
    ctx.write_cooldowns.insert("a.rs".to_string(), 2);
    ctx.write_cooldowns.insert("b.rs".to_string(), 1);
    ctx.decrement_cooldowns();
    assert_eq!(ctx.write_cooldowns.get("a.rs"), Some(&1));
    assert!(!ctx.write_cooldowns.contains_key("b.rs"));
}

#[test]
fn test_detect_missing_args_blocks_write_file_without_path() {
    let ctx = BlockingContext::new(12);
    let tool = make_tool("write_file", serde_json::json!({}));
    let result = detect_missing_required_args(&tool, &ctx).unwrap();
    assert!(result.blocked);
    let msg = result.recovery_message.unwrap();
    assert!(msg.contains("requires a non-empty `path`"));
    assert!(
        msg.contains("write_file(path="),
        "block message must include a concrete example"
    );
}

#[test]
fn test_detect_missing_args_blocks_edit_file_without_path() {
    let ctx = BlockingContext::new(12);
    let tool = make_tool("edit_file", serde_json::json!({}));
    let result = detect_missing_required_args(&tool, &ctx).unwrap();
    assert!(result.blocked);
    assert!(result
        .recovery_message
        .unwrap()
        .contains("edit_file(path="));
}

#[test]
fn test_detect_missing_args_blocks_delete_file_without_path() {
    let ctx = BlockingContext::new(12);
    let tool = make_tool("delete_file", serde_json::json!({}));
    let result = detect_missing_required_args(&tool, &ctx).unwrap();
    assert!(result.blocked);
}

#[test]
fn test_detect_missing_args_allows_write_file_with_path() {
    let ctx = BlockingContext::new(12);
    let tool = make_tool("write_file", serde_json::json!({"path": "test.rs"}));
    let result = detect_missing_required_args(&tool, &ctx);
    assert!(result.is_none());
}

#[test]
fn test_detect_missing_args_blocks_write_file_with_empty_path_string() {
    let ctx = BlockingContext::new(12);
    let tool = make_tool("write_file", serde_json::json!({"path": ""}));
    let result = detect_missing_required_args(&tool, &ctx).unwrap();
    assert!(result.blocked);
    assert!(result
        .recovery_message
        .as_deref()
        .unwrap()
        .contains("non-empty `path`"));
}

#[test]
fn test_detect_missing_args_blocks_edit_file_with_whitespace_path() {
    let ctx = BlockingContext::new(12);
    let tool = make_tool("edit_file", serde_json::json!({"path": "   \t"}));
    let result = detect_missing_required_args(&tool, &ctx).unwrap();
    assert!(result.blocked);
}

#[test]
fn test_detect_missing_args_blocks_read_file_with_empty_path() {
    let ctx = BlockingContext::new(12);
    let tool = make_tool("read_file", serde_json::json!({"path": ""}));
    let result = detect_missing_required_args(&tool, &ctx).unwrap();
    assert!(result.blocked);
}

#[test]
fn test_detect_missing_args_uses_last_read_path_as_hint() {
    let mut ctx = BlockingContext::new(12);
    ctx.on_read_path("crates/zero-identity/src/identity.rs");
    let tool = make_tool("edit_file", serde_json::json!({}));
    let msg = detect_missing_required_args(&tool, &ctx)
        .unwrap()
        .recovery_message
        .unwrap();
    assert!(
        msg.contains("crates/zero-identity/src/identity.rs"),
        "hint from last-read path should appear in example, got: {msg}"
    );
    assert!(msg.contains("Definition-of-Done gate"));
}

#[test]
fn test_detect_missing_args_blocks_run_command_without_command() {
    let ctx = BlockingContext::new(12);
    let tool = make_tool("run_command", serde_json::json!({}));
    let result = detect_missing_required_args(&tool, &ctx).unwrap();
    assert!(result.blocked);
    assert!(result
        .recovery_message
        .unwrap()
        .contains("requires a `command`"));
}

#[test]
fn test_detect_missing_args_blocks_run_command_with_empty_command() {
    let ctx = BlockingContext::new(12);
    let tool = make_tool("run_command", serde_json::json!({"command": "  "}));
    let result = detect_missing_required_args(&tool, &ctx).unwrap();
    assert!(result.blocked);
}

#[test]
fn test_detect_missing_args_allows_run_command_with_command() {
    let ctx = BlockingContext::new(12);
    let tool = make_tool("run_command", serde_json::json!({"command": "cargo build"}));
    let result = detect_missing_required_args(&tool, &ctx);
    assert!(result.is_none());
}

#[test]
fn test_detect_missing_args_blocks_read_file_without_path() {
    let ctx = BlockingContext::new(12);
    let tool = make_tool("read_file", serde_json::json!({}));
    let result = detect_missing_required_args(&tool, &ctx).unwrap();
    assert!(result.blocked);
}

#[test]
fn test_detect_missing_args_skips_unrelated_tools() {
    let ctx = BlockingContext::new(12);
    let tool = make_tool("list_files", serde_json::json!({}));
    let result = detect_missing_required_args(&tool, &ctx);
    assert!(result.is_none());
}

#[test]
fn test_pathless_write_hint_prefers_last_read_then_written() {
    let mut ctx = BlockingContext::new(12);
    assert!(ctx.pathless_write_hint().is_none());
    ctx.written_paths.insert("src/lib.rs".into());
    assert_eq!(ctx.pathless_write_hint(), Some("src/lib.rs"));
    ctx.on_read_path("src/main.rs");
    assert_eq!(
        ctx.pathless_write_hint(),
        Some("src/main.rs"),
        "last-read path must take precedence over written fallback"
    );
}

#[test]
fn test_detect_all_blocked_catches_empty_args_write() {
    let ctx = BlockingContext::new(12);
    let read_guard = ReadGuardState::default();
    let tool = make_tool("delete_file", serde_json::json!({}));
    let result = detect_all_blocked(&tool, &ctx, &read_guard);
    assert!(result.blocked);
}

#[test]
fn test_detect_all_blocked_combines_all_detectors() {
    let ctx = BlockingContext::new(12);
    let read_guard = ReadGuardState::default();
    let tool = make_tool("write_file", serde_json::json!({"path": "new.rs"}));
    let result = detect_all_blocked(&tool, &ctx, &read_guard);
    assert!(!result.blocked);
}

#[test]
fn test_on_write_success_resets_state() {
    let mut ctx = BlockingContext::new(12);
    let mut read_guard = ReadGuardState::default();
    read_guard.record_full_read("test.rs");
    ctx.write_failures.insert("test.rs".to_string(), 2);
    ctx.on_write_success("test.rs", &mut read_guard);
    assert!(ctx.written_paths.contains("test.rs"));
    assert!(!ctx.write_failures.contains_key("test.rs"));
    assert_eq!(ctx.exploration_allowance, 14);
    assert_eq!(read_guard.full_read_count("test.rs"), 0);
}
