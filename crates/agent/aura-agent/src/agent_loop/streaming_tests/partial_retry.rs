use super::*;

#[tokio::test]
#[allow(clippy::await_holding_lock)] // intentional: serializes env var edits across async awaits
async fn stream_aborted_with_partial_retries_then_succeeds() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(5, 1, 2);

    let provider = FlakyPartialProvider::new(2, "recovered");
    let executor = NoOpExecutor;
    let agent = AgentLoop::new(pump_config());
    let (tx, rx) = mpsc::channel(1024);
    let messages = vec![Message::user("hello")];

    let result = agent
        .run_with_events(&provider, &executor, messages, vec![], Some(tx), None)
        .await
        .expect("retry should eventually succeed");
    assert_eq!(result.iterations, 1);

    let events = collect_events(rx).await;
    let retrying = events
        .iter()
        .filter(|e| matches!(e, AgentLoopEvent::ToolCallRetrying { .. }))
        .count();
    assert_eq!(
        retrying, 2,
        "expected exactly two ToolCallRetrying events, got: {retrying}"
    );

    let failed = events
        .iter()
        .any(|e| matches!(e, AgentLoopEvent::ToolCallFailed { .. }));
    assert!(!failed, "success path must not emit ToolCallFailed");

    let any_write_file_retry = events.iter().any(|e| match e {
        AgentLoopEvent::ToolCallRetrying { tool_name, .. } => tool_name == "write_file",
        _ => false,
    });
    assert!(
        any_write_file_retry,
        "retry events should carry the original tool_name (write_file)"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // intentional: serializes env var edits across async awaits
async fn stream_aborted_with_partial_exhausts_and_fails() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(2, 1, 2);

    let provider = FlakyPartialProvider::new(1_000, "never-used");
    let executor = NoOpExecutor;
    let agent = AgentLoop::new(pump_config());
    let (tx, rx) = mpsc::channel(1024);
    let messages = vec![Message::user("hello")];

    let result = agent
        .run_with_events(&provider, &executor, messages, vec![], Some(tx), None)
        .await;
    let _ = result;

    let events = collect_events(rx).await;
    let retrying = events
        .iter()
        .filter(|e| matches!(e, AgentLoopEvent::ToolCallRetrying { .. }))
        .count();
    assert_eq!(
        retrying, 2,
        "expected two retries before exhaustion, got: {retrying}"
    );

    let failed = events
        .iter()
        .filter(|e| matches!(e, AgentLoopEvent::ToolCallFailed { .. }))
        .count();
    assert_eq!(
        failed, 1,
        "expected exactly one ToolCallFailed after exhaustion, got: {failed}"
    );
}
