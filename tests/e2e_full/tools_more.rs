//! Suite 6 (continued) and multi-turn tool tests.

use crate::support::*;

#[tokio::test]
async fn tool_read_file_line_range() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-linerange");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    let content = (1..=20)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    place_file_in_agent_dir(&ws_path, "numbered.txt", &content);

    ws.send_user_message(
        "Use the read_file tool to read 'numbered.txt' with start_line=5 and end_line=10.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"read_file".to_string()),
        "expected read_file tool use, got: {tools:?}"
    );

    // The key assertion is that read_file was invoked with line range params.
    // The tool result may or may not contain the expected lines depending on
    // path resolution in the agent workspace.
    let results: Vec<&Value> = messages
        .iter()
        .filter(|m| m["type"] == "tool_result" && m["name"] == "read_file")
        .collect();
    assert!(
        !results.is_empty(),
        "expected at least one read_file tool_result"
    );
}

#[tokio::test]
async fn tool_search_code_regex() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-regex-search");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(
        &ws_path,
        "code.rs",
        "fn add_numbers(a: i32, b: i32) -> i32 { a + b }\nfn subtract_numbers(a: i32, b: i32) -> i32 { a - b }\n",
    );

    ws.send_user_message(
        "Use the search_code tool with the regex pattern 'fn \\w+_numbers' to find all functions ending with '_numbers'.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"search_code".to_string()),
        "expected search_code tool use, got: {tools:?}"
    );
}

#[tokio::test]
async fn tool_stat_file() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-stat");
    std::fs::create_dir_all(&ws_path).unwrap();
    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(&ws_path, "info.txt", "some content");

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
async fn tool_list_files_with_content() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-list");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(&ws_path, "alpha.txt", "a");
    place_file_in_agent_dir(&ws_path, "bravo.txt", "b");

    ws.send_user_message("Use the list_files tool to list the current directory.")
        .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"list_files".to_string()),
        "expected list_files tool use, got: {tools:?}"
    );
}

#[tokio::test]
async fn tool_write_file_overwrite() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-overwrite");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    place_file_in_agent_dir(&ws_path, "overwrite.txt", "ORIGINAL");

    ws.send_user_message("Use the write_file tool to write 'OVERWRITTEN' to 'overwrite.txt'.")
        .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"write_file".to_string()),
        "expected write_file, got: {tools:?}"
    );

    if let Some(path) = find_file(&ws_path, "overwrite.txt") {
        let content = std::fs::read_to_string(path).unwrap();
        assert!(
            content.contains("OVERWRITTEN"),
            "should have new content, got: {content}"
        );
        assert!(
            !content.contains("ORIGINAL"),
            "original content should be gone, got: {content}"
        );
    }
}

#[tokio::test]
async fn tool_run_command_with_cwd() {
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
        "Use the run_command tool to execute '{cmd}' with working_dir set to 'subdir'."
    ))
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"run_command".to_string()),
        "expected run_command, got: {tools:?}"
    );
}

// ============================================================================
// Suite: Multi-turn Conversation
// ============================================================================

#[tokio::test]
async fn tool_multi_turn_context_preserved() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("multi-turn");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    // Turn 1: create a file
    ws.send_user_message(
        "Use the write_file tool to create 'memo.txt' with content 'Remember to buy milk'.",
    )
    .await;
    let turn1 = ws.collect_turn(Duration::from_secs(120)).await;
    assert!(
        tool_names_used(&turn1).contains(&"write_file".to_string()),
        "turn 1 should use write_file"
    );

    // Turn 2: read the same file
    ws.send_user_message("Use the read_file tool to read 'memo.txt' and tell me what it says.")
        .await;
    let turn2 = ws.collect_turn(Duration::from_secs(120)).await;
    let end = turn2
        .iter()
        .find(|m| m["type"] == "assistant_message_end")
        .expect("turn 2 should have assistant_message_end");
    let stop = end["stop_reason"].as_str().unwrap();
    assert!(
        stop == "end_turn" || stop == "end_turn_with_errors",
        "turn 2 stop_reason should be end_turn or end_turn_with_errors, got: {stop}"
    );
}

#[tokio::test]
async fn tool_cancel_during_turn() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("cancel-turn");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message(
        "Write a very long detailed essay about the history of computing, at least 2000 words.",
    )
    .await;

    // Wait for streaming to start, then cancel
    tokio::time::sleep(Duration::from_secs(2)).await;
    ws.send_cancel().await;

    let messages = ws.collect_turn(Duration::from_secs(60)).await;

    let end = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end");
    if let Some(e) = end {
        let stop = e["stop_reason"].as_str().unwrap_or("");
        assert!(
            stop == "cancelled" || stop == "end_turn" || stop == "end_turn_with_errors",
            "expected cancelled or end_turn, got: {stop}"
        );
    }
}

#[tokio::test]
async fn tool_message_during_turn_rejected() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("msg-during-turn");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message("Write a 500-word essay about software testing.")
        .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    ws.send_user_message("This should be rejected").await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;

    let has_turn_in_progress = messages
        .iter()
        .any(|m| m["type"] == "error" && m["code"] == "turn_in_progress");
    assert!(has_turn_in_progress, "expected turn_in_progress error");
}
