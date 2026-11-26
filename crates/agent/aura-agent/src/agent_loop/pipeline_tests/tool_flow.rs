use super::*;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pipeline_full_tool_execution_flow() {
    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "tool_1",
            "read_file",
            serde_json::json!({"path": "test.txt"}),
        ))
        .with_response(MockResponse::text("done"));

    let executor = SuccessExecutor;
    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("Read test.txt")];
    let tools = vec![read_file_tool()];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(result.iterations, 2);

    let has_tool_result = result.messages.iter().any(|msg| {
        msg.content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
    });
    assert!(has_tool_result, "Messages must contain a tool_result block");
    assert!(
        result.total_text.contains("done"),
        "Final text must contain 'done'"
    );
}

#[tokio::test]
async fn pipeline_write_success_clears_cache() {
    let counter = Arc::new(AtomicUsize::new(0));
    let executor = CountingExecutor {
        call_count: Arc::clone(&counter),
    };

    // Phase 1B: a successful write only invalidates cache entries
    // whose path overlaps the written one. To keep the original
    // "write clears the read cache, so the second read_file
    // re-executes" assertion, target the write at the same path
    // the read cached. The path-isolation half of the new contract
    // is covered by `write_invalidates_only_overlapping_path` in
    // `tool_execution_tests`.
    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "t1",
            "read_file",
            serde_json::json!({"path": "test.txt"}),
        ))
        .with_response(MockResponse::tool_use(
            "t2",
            "write_file",
            serde_json::json!({"path": "test.txt", "content": "fn main() {}"}),
        ))
        .with_response(MockResponse::tool_use(
            "t3",
            "read_file",
            serde_json::json!({"path": "test.txt"}),
        ))
        .with_response(MockResponse::text("done"));

    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("read, write, read again")];
    let tools = vec![read_file_tool(), write_file_tool()];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(result.iterations, 4);
    assert_eq!(
        counter.load(Ordering::SeqCst),
        3,
        "read_file executed twice (cache cleared by write to same path) + one write = 3 total"
    );
}

#[tokio::test]
async fn pipeline_every_tool_emits_result_event() {
    let inner = MockProvider::new()
        .with_response(MockResponse {
            stop_reason: StopReason::ToolUse,
            content: vec![
                ContentBlock::tool_use("t1", "read_file", serde_json::json!({"path": "a.txt"})),
                ContentBlock::tool_use("t2", "read_file", serde_json::json!({"path": "b.txt"})),
            ],
            usage: Usage::new(100, 50),
        })
        .with_response(MockResponse::text("done"));

    let provider = StreamingMockProvider { inner };
    let executor = SuccessExecutor;
    let agent = AgentLoop::new(default_config());

    let (tx, rx) = mpsc::channel(1024);
    let messages = vec![Message::user("read two files")];
    let tools = vec![read_file_tool()];

    let result = agent
        .run_with_events(&provider, &executor, messages, tools, Some(tx), None)
        .await
        .unwrap();

    assert_eq!(result.iterations, 2);

    let events = collect_events(rx).await;
    let tool_result_count = events
        .iter()
        .filter(|e| matches!(e, AgentLoopEvent::ToolResult { .. }))
        .count();

    assert_eq!(
        tool_result_count, 2,
        "Each tool execution must emit a ToolResult event"
    );
}
