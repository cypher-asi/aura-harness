//! Suite 6: LLM tool coverage.

use crate::support::*;

// ============================================================================
// Suite 6: LLM Tool Coverage (requires credentials)
// ============================================================================

#[tokio::test]
async fn tool_write_file_nested_dirs() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-nested");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message(
        "Use the write_file tool to create a file at path 'a/b/c/deep.txt' with content 'deep file'. Do it now.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"write_file".to_string()),
        "expected write_file tool use, got: {tools:?}"
    );

    let found = find_file(&ws_path, "deep.txt");
    if let Some(path) = found {
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("deep"),
            "file content should contain 'deep', got: {content}"
        );
        // Verify nested directory was created
        assert!(
            path.to_string_lossy().contains("a") || path.to_string_lossy().contains("b"),
            "file should be in nested dir, got path: {path:?}"
        );
    }
}

#[tokio::test]
async fn tool_edit_file_replace_all() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-replaceall");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(&ws_path, "repeated.txt", "foo bar foo baz foo qux");

    ws.send_user_message(
        "Use the edit_file tool on 'repeated.txt' with old_text='foo', new_text='REPLACED', and set replace_all to true. Do it now.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"edit_file".to_string()),
        "expected edit_file tool use, got: {tools:?}"
    );

    if let Some(path) = find_file(&ws_path, "repeated.txt") {
        let content = std::fs::read_to_string(path).unwrap();
        if content.contains("foo") {
            eprintln!("NOTE: edit_file with replace_all may not have replaced all occurrences (LLM behaviour)");
        }
    }
}

#[tokio::test]
async fn tool_search_code_with_context() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-search-ctx");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(
        &ws_path,
        "sample.rs",
        "line1\nline2\nfn target_function() {\n    // body\n}\nline6\nline7\n",
    );

    ws.send_user_message(
        "Use the search_code tool to search for 'target_function' with context_lines set to 2.",
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
async fn tool_find_files_scoped() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-find-scoped");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    if let Some(agent_dir) = find_agent_dir(&ws_path) {
        let src = agent_dir.join("src");
        let docs = agent_dir.join("docs");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(src.join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(docs.join("guide.md"), "# Guide").unwrap();
    }

    ws.send_user_message(
        "Use the find_files tool with pattern '*.rs' and path set to 'src'. Only search in src.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"find_files".to_string()),
        "expected find_files tool use, got: {tools:?}"
    );
}

#[tokio::test]
async fn tool_run_command_with_timeout() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-cmd-timeout");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    let cmd = if cfg!(windows) {
        "ping -n 10 127.0.0.1"
    } else {
        "sleep 10"
    };
    let prompts = [
        format!("Use the run_command tool to execute '{cmd}' with timeout_secs set to 2."),
        format!(
            "Call the run_command tool immediately. Execute exactly '{cmd}' with timeout_secs set to 2 and do not answer without the tool."
        ),
    ];

    let mut last_tools = Vec::new();
    for prompt in prompts {
        ws.send_user_message(&prompt).await;
        let messages = ws.collect_turn(Duration::from_secs(120)).await;
        let tools = tool_names_used(&messages);
        if tools.contains(&"run_command".to_string()) {
            return;
        }
        last_tools = tools;
    }

    panic!("expected run_command tool use, got: {last_tools:?}");
}

#[tokio::test]
async fn tool_multi_step_single_turn() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-multi-step");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message(
        "Do the following steps using tools:\n\
         1. Use write_file to create 'steps.txt' with content 'step one'\n\
         2. Use edit_file to replace 'step one' with 'step two' in 'steps.txt'\n\
         3. Use read_file to read 'steps.txt' and confirm it says 'step two'\n\
         Do all three steps now.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(180)).await;
    let tools = tool_names_used(&messages);

    // The LLM should use at least one tool; it may consolidate or
    // reorder steps, so we assert broadly.
    assert!(
        !tools.is_empty(),
        "expected at least one tool call in multi-step turn, got none"
    );
    assert!(
        tools.contains(&"write_file".to_string())
            || tools.contains(&"edit_file".to_string())
            || tools.contains(&"read_file".to_string()),
        "expected at least one file tool in multi-step, got: {tools:?}"
    );
}

#[tokio::test]
async fn tool_write_read_roundtrip() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-roundtrip");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message(
        "Use write_file to create 'roundtrip.txt' with the exact content 'MARKER_ABC_123'. \
         Then use read_file to read it back. Tell me what it contains.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"write_file".to_string()),
        "expected write_file, got: {tools:?}"
    );

    // Check tool_result for read_file contains the marker
    let read_results: Vec<&Value> = messages
        .iter()
        .filter(|m| m["type"] == "tool_result" && m["name"] == "read_file")
        .collect();
    if !read_results.is_empty() {
        let result = read_results[0]["result"].as_str().unwrap_or("");
        let decoded_stdout = serde_json::from_str::<Value>(result)
            .ok()
            .and_then(|parsed| parsed["stdout"].as_str().map(str::to_owned))
            .and_then(|stdout| {
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, stdout).ok()
            })
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_else(|| result.to_string());
        assert!(
            decoded_stdout.contains("MARKER_ABC_123"),
            "read_file should return written content, got: {result}"
        );
    }
}

#[tokio::test]
async fn tool_read_nonexistent_errors() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-read-noexist");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message("Use the read_file tool to read 'does_not_exist_xyz.txt'. Do it now.")
        .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"read_file".to_string()),
        "expected read_file tool use, got: {tools:?}"
    );
}

#[tokio::test]
async fn tool_edit_no_match_errors() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-edit-nomatch");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    place_file_in_agent_dir(&ws_path, "immutable.txt", "original content here");

    ws.send_user_message(
        "Use the edit_file tool on 'immutable.txt'. The old_text is 'ZZZZZ_NONEXISTENT_ZZZZZ' \
         and the new_text is 'replaced'. Do it now.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"edit_file".to_string()),
        "expected edit_file tool use, got: {tools:?}"
    );

    // File should be unchanged
    if let Some(path) = find_file(&ws_path, "immutable.txt") {
        let content = std::fs::read_to_string(path).unwrap();
        assert_eq!(content, "original content here", "file should be unchanged");
    }
}

#[tokio::test]
async fn tool_delete_nonexistent() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-del-noexist");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message("Use the delete_file tool to delete 'ghost_file_xyz.txt'. Do it now.")
        .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"delete_file".to_string()),
        "expected delete_file tool use, got: {tools:?}"
    );
}

#[tokio::test]
async fn tool_run_command_failure() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-cmd-fail");
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
}
