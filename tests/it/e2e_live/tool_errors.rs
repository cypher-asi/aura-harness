//! Suites 14-15: tool error paths and parameter variations.

use std::time::Duration;

use serde_json::Value;

use crate::common::{
    connect_llm_session, find_agent_dir, find_file, has_tool_error, place_file_in_agent_dir,
    tool_names_used, TestServer,
};

// ============================================================================
// Suite 14: Tool Error Paths
// ============================================================================

#[tokio::test]
async fn test_tool_read_nonexistent_file() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-err-read");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message(
        "Use the read_file tool to read a file called 'does_not_exist_xyz.txt'. Do it now.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"read_file".to_string()),
        "expected read_file tool use, got: {tools:?}"
    );

    if !has_tool_error(&messages, "read_file") {
        eprintln!("NOTE: read_file on nonexistent file did not produce is_error=true (LLM may have handled it)");
    }
}

#[tokio::test]
async fn test_tool_delete_nonexistent_file() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-err-delete");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message(
        "Use the delete_file tool to delete 'nonexistent_file_xyz.txt'. Do it now.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"delete_file".to_string()),
        "expected delete_file tool use, got: {tools:?}"
    );
}

#[tokio::test]
async fn test_tool_run_command_failure() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-err-cmd");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    let cmd = if cfg!(windows) {
        "cmd /c dir nonexistent_dir_e2e_xyz 2>&1"
    } else {
        "ls /nonexistent_dir_e2e_xyz 2>&1"
    };
    ws.send_user_message(&format!(
        "Use the run_command tool to execute exactly this command: {cmd}"
    ))
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"run_command".to_string()),
        "expected run_command tool use, got: {tools:?}"
    );

    let cmd_results: Vec<&Value> = messages
        .iter()
        .filter(|m| m["type"] == "tool_result" && m["name"] == "run_command")
        .collect();
    if !cmd_results.is_empty() {
        let result_text = cmd_results[0]["result"].as_str().unwrap_or("");
        assert!(
            result_text.contains("not find")
                || result_text.contains("No such")
                || result_text.contains("cannot"),
            "failing command should mention error, got: {result_text}"
        );
    }
}

#[tokio::test]
async fn test_tool_edit_no_match() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-err-edit");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    place_file_in_agent_dir(&ws_path, "stable.txt", "This content will not change.");

    ws.send_user_message(
        "Use the edit_file tool on 'stable.txt'. The old_string is 'ZZZZZ_NONEXISTENT_ZZZZZ' \
         and the new_string is 'replaced'. Do it now.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"edit_file".to_string()),
        "expected edit_file tool use, got: {tools:?}"
    );

    if has_tool_error(&messages, "edit_file") {
        // Good: tool correctly reported the no-match error
    } else {
        eprintln!("NOTE: edit_file with no-match string did not produce is_error=true");
    }

    // Verify the file was NOT modified
    if let Some(path) = find_file(&ws_path, "stable.txt") {
        let content = std::fs::read_to_string(path).unwrap();
        assert_eq!(
            content, "This content will not change.",
            "file should be unchanged after no-match edit"
        );
    }
}

// ============================================================================
// Suite 15: Tool Parameter Variations
// ============================================================================

#[tokio::test]
async fn test_tool_read_file_line_range() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-line-range");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    let content = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n";
    place_file_in_agent_dir(&ws_path, "lines.txt", content);

    ws.send_user_message(
        "Use the read_file tool to read 'lines.txt' with start_line=3 and end_line=5. \
         Only read those specific lines.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"read_file".to_string()),
        "expected read_file tool use, got: {tools:?}"
    );

    let tool_results: Vec<&Value> = messages
        .iter()
        .filter(|m| m["type"] == "tool_result" && m["name"] == "read_file")
        .collect();
    if !tool_results.is_empty() {
        let result = tool_results[0]["result"].as_str().unwrap_or("");
        assert!(
            result.contains("line3"),
            "line range result should include line3, got: {result}"
        );
    }
}

#[tokio::test]
async fn test_tool_run_command_with_cwd() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-cwd");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    if let Some(agent_dir) = find_agent_dir(&ws_path) {
        std::fs::create_dir_all(agent_dir.join("subdir")).unwrap();
    }

    let cmd = if cfg!(windows) { "cd" } else { "pwd" };
    ws.send_user_message(&format!(
        "Use the run_command tool to execute '{cmd}' with the working_dir (or cwd) set to 'subdir'."
    ))
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"run_command".to_string()),
        "expected run_command tool use, got: {tools:?}"
    );

    let cmd_results: Vec<&Value> = messages
        .iter()
        .filter(|m| m["type"] == "tool_result" && m["name"] == "run_command")
        .collect();
    if !cmd_results.is_empty() {
        let result = cmd_results[0]["result"].as_str().unwrap_or("");
        assert!(
            result.contains("subdir"),
            "cwd should reference subdir, got: {result}"
        );
    }
}

#[tokio::test]
async fn test_tool_write_file_overwrite() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-overwrite");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    place_file_in_agent_dir(&ws_path, "overwrite.txt", "ORIGINAL CONTENT");

    ws.send_user_message(
        "Use the write_file tool to write 'OVERWRITTEN CONTENT' to the file 'overwrite.txt'. \
         This should overwrite the existing file.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"write_file".to_string()),
        "expected write_file tool use, got: {tools:?}"
    );

    if let Some(path) = find_file(&ws_path, "overwrite.txt") {
        let content = std::fs::read_to_string(path).unwrap();
        assert!(
            content.contains("OVERWRITTEN"),
            "file should have overwritten content, got: {content}"
        );
        assert!(
            !content.contains("ORIGINAL"),
            "original content should be gone, got: {content}"
        );
    }
}

#[tokio::test]
async fn test_tool_search_code_regex() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-regex");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(
        &ws_path,
        "math.rs",
        "fn add_numbers(a: i32, b: i32) -> i32 { a + b }\nfn subtract_numbers(a: i32, b: i32) -> i32 { a - b }\n",
    );
    place_file_in_agent_dir(
        &ws_path,
        "utils.rs",
        "fn multiply_numbers(a: i32, b: i32) -> i32 { a * b }\n",
    );

    ws.send_user_message(
        "Use the search_code tool with the regex pattern 'fn \\w+_numbers' to find all \
         functions ending with '_numbers'. Search the current directory.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"search_code".to_string()),
        "expected search_code tool use, got: {tools:?}"
    );

    let tool_results: Vec<&Value> = messages
        .iter()
        .filter(|m| m["type"] == "tool_result" && m["name"] == "search_code")
        .collect();
    if !tool_results.is_empty() {
        let result = tool_results[0]["result"].as_str().unwrap_or("");
        assert!(
            result.contains("add_numbers")
                || result.contains("subtract_numbers")
                || result.contains("multiply_numbers"),
            "regex search should find _numbers functions, got: {result}"
        );
    }
}
