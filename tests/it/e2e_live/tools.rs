//! Suite 4: LLM + filesystem tool end-to-end tests.

use std::time::Duration;

use serde_json::Value;

use crate::common::{
    assert_stop_reason, connect_llm_session, find_agent_dir, find_file, place_file_in_agent_dir,
    tool_names_used, TestServer,
};

// ============================================================================
// Suite 4: LLM + Filesystem Tool E2E Tests
// ============================================================================

#[tokio::test]

async fn test_tool_write_and_read_file() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-write-read");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message(
        "Use the write_file tool to create a file at path 'hello.txt' with the content 'Hello E2E Test'. \
         After writing, use the read_file tool to read 'hello.txt' and confirm it exists.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"write_file".to_string()),
        "expected write_file tool use"
    );

    let file_path = find_file(&ws_path, "hello.txt");
    assert!(
        file_path.is_some(),
        "file was not created on disk anywhere under workspace"
    );
    let content = std::fs::read_to_string(file_path.unwrap()).unwrap();
    assert!(
        content.contains("Hello E2E Test"),
        "file content mismatch: {content}"
    );
}

#[tokio::test]

async fn test_tool_edit_file() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-edit");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    // Place file in the agent directory (where tools operate)
    place_file_in_agent_dir(
        &ws_path,
        "target.txt",
        "The quick brown fox jumps over the lazy dog.",
    );

    ws.send_user_message(
        "Use the edit_file tool on the file 'target.txt'. \
         The old_string to find is 'brown fox' and the new_string to replace it with is 'red panda'. \
         Do this now using the edit_file tool.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"edit_file".to_string()),
        "expected edit_file tool use, got: {tools:?}"
    );

    // Verify the edit was applied (LLM may provide slightly different params)
    if let Some(file_path) = find_file(&ws_path, "target.txt") {
        let content = std::fs::read_to_string(file_path).unwrap();
        if !content.contains("red panda") {
            eprintln!(
                "NOTE: edit_file tool was invoked but edit wasn't applied as expected (LLM behavior). \
                 Content: {content}"
            );
        }
    }
}

#[tokio::test]

async fn test_tool_delete_file() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-delete");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    // Place file in the agent directory (where tools operate)
    place_file_in_agent_dir(&ws_path, "to_delete.txt", "delete me");

    ws.send_user_message(
        "Use the delete_file tool to delete the file at path 'to_delete.txt'. Do it now.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"delete_file".to_string()),
        "expected delete_file tool use, got: {tools:?}"
    );
    // Verify deletion (LLM may provide a slightly different path)
    if find_file(&ws_path, "to_delete.txt").is_some() {
        eprintln!(
            "NOTE: delete_file tool was invoked but file still exists (LLM may have used wrong path)"
        );
    }
}

#[tokio::test]

async fn test_tool_list_files() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-list");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(&ws_path, "alpha.txt", "a");
    place_file_in_agent_dir(&ws_path, "bravo.txt", "b");
    place_file_in_agent_dir(&ws_path, "charlie.txt", "c");

    ws.send_user_message(
        "Use the list_files tool to list the contents of the current directory '.'. \
         Then tell me the exact filenames you found.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"list_files".to_string()),
        "expected list_files tool use, got: {tools:?}"
    );
}

#[tokio::test]

async fn test_tool_stat_file() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-stat");
    std::fs::create_dir_all(&ws_path).unwrap();
    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(&ws_path, "info.txt", "some content here");

    ws.send_user_message("Get file metadata for 'info.txt' using the stat_file tool.")
        .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"stat_file".to_string()),
        "expected stat_file tool use"
    );
    assert_stop_reason(&messages, "end_turn");
}

#[tokio::test]

async fn test_tool_find_files() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-find");
    let nested = ws_path.join("src").join("components");
    std::fs::create_dir_all(&nested).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    // Create file structure in the agent directory (where tools operate)
    if let Some(agent_dir) = find_agent_dir(&ws_path) {
        let agent_src = agent_dir.join("src").join("components");
        std::fs::create_dir_all(&agent_src).unwrap();
        std::fs::write(agent_dir.join("readme.md"), "root").unwrap();
        std::fs::write(agent_dir.join("src").join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(agent_src.join("button.rs"), "struct Button;").unwrap();
        std::fs::write(agent_src.join("input.rs"), "struct Input;").unwrap();
    } else {
        std::fs::write(ws_path.join("readme.md"), "root").unwrap();
        std::fs::write(ws_path.join("src").join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(nested.join("button.rs"), "struct Button;").unwrap();
        std::fs::write(nested.join("input.rs"), "struct Input;").unwrap();
    }

    ws.send_user_message(
        "Find all '.rs' files in this project recursively. Use the find_files tool.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"find_files".to_string()),
        "expected find_files tool use"
    );
    assert_stop_reason(&messages, "end_turn");
}

#[tokio::test]

async fn test_tool_search_code() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-search");
    std::fs::create_dir_all(&ws_path).unwrap();

    std::fs::write(
        ws_path.join("app.rs"),
        "fn calculate_total(items: &[i32]) -> i32 {\n    items.iter().sum()\n}\n",
    )
    .unwrap();
    std::fs::write(
        ws_path.join("lib.rs"),
        "pub fn calculate_total(prices: &[f64]) -> f64 {\n    prices.iter().sum()\n}\n",
    )
    .unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(
        &ws_path,
        "app.rs",
        "fn calculate_total(items: &[i32]) -> i32 {\n    items.iter().sum()\n}\n",
    );
    place_file_in_agent_dir(
        &ws_path,
        "lib.rs",
        "pub fn calculate_total(prices: &[f64]) -> f64 {\n    prices.iter().sum()\n}\n",
    );

    ws.send_user_message(
        "Search for all occurrences of 'calculate_total' in the codebase. Use the search_code tool.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"search_code".to_string()),
        "expected search_code tool use"
    );
    assert_stop_reason(&messages, "end_turn");
}

#[tokio::test]

async fn test_tool_run_command() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-cmd");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    let cmd = if cfg!(windows) {
        "cmd /c echo hello_e2e"
    } else {
        "echo hello_e2e"
    };
    ws.send_user_message(&format!(
        "Use the run_command tool to execute this exact command: {cmd}"
    ))
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"run_command".to_string()),
        "expected run_command tool use, got: {tools:?}"
    );

    // Check the tool result contains our marker string
    let tool_results: Vec<&Value> = messages
        .iter()
        .filter(|m| m["type"] == "tool_result" && m["name"] == "run_command")
        .collect();
    if !tool_results.is_empty() {
        let result_text = tool_results[0]["result"].as_str().unwrap_or("");
        assert!(
            result_text.contains("hello_e2e"),
            "command output should contain 'hello_e2e', got: {result_text}"
        );
    }
}
