use super::*;

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn stream_connection_reset_recovers_and_discards_partial_output() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(2, 1, 2);
    let provider = DisconnectProvider::new(1, DisconnectKind::BodyReset);
    let (tx, rx) = mpsc::channel(1024);
    let result = AgentLoop::new(pump_config())
        .run_with_events(
            &provider,
            &NoOpExecutor,
            vec![Message::user("hello")],
            vec![],
            Some(tx),
            None,
        )
        .await
        .expect("recovered run");
    assert!(result.llm_error.is_none(), "{:?}", result.llm_error);
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    let events = collect_events(rx).await;
    let reset = events
        .iter()
        .position(|e| matches!(e, AgentLoopEvent::StreamReset { .. }))
        .expect("reset partial UI content");
    assert!(
        matches!(&events[reset], AgentLoopEvent::StreamReset { text_bytes, thinking_bytes, .. }
        if *text_bytes == "discard this partial answer".len() && *thinking_bytes == 0)
    );
    assert!(events[..reset]
        .iter()
        .any(|e| matches!(e, AgentLoopEvent::TextDelta(t) if t.contains("discard"))));
    assert!(events[reset + 1..]
        .iter()
        .any(|e| matches!(e, AgentLoopEvent::TextDelta(t) if t == "recovered answer")));
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentLoopEvent::ToolCallFailed { .. })));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn stream_connection_reset_exhausts_bounded_budget() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(2, 1, 2);
    let provider = DisconnectProvider::new(usize::MAX, DisconnectKind::BodyReset);
    let (tx, rx) = mpsc::channel(1024);
    let result = AgentLoop::new(pump_config())
        .run_with_events(
            &provider,
            &NoOpExecutor,
            vec![Message::user("hello")],
            vec![],
            Some(tx),
            None,
        )
        .await
        .expect("errors carried by loop result");
    assert!(result
        .llm_error
        .as_deref()
        .is_some_and(|s| s.contains("10054")));
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    let events = collect_events(rx).await;
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentLoopEvent::StreamReset { .. }))
            .count(),
        2
    );
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentLoopEvent::ToolCallFailed { .. })));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn stream_open_connection_error_recovers() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(2, 1, 2);
    let provider = DisconnectProvider::new(1, DisconnectKind::OpenRequest);
    let result = AgentLoop::new(pump_config())
        .run(
            &provider,
            &NoOpExecutor,
            vec![Message::user("hello")],
            vec![],
        )
        .await
        .expect("recovered request");
    assert!(result.llm_error.is_none());
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn stream_invalid_event_is_not_retried() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(2, 1, 2);
    let provider = DisconnectProvider::new(1, DisconnectKind::InvalidEvent);
    let result = AgentLoop::new(pump_config())
        .run(
            &provider,
            &NoOpExecutor,
            vec![Message::user("hello")],
            vec![],
        )
        .await
        .expect("errors carried by loop result");
    assert!(result.llm_error.is_some());
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn stream_disconnect_after_completed_tool_is_not_replayed() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(2, 1, 2);
    for kind in [DisconnectKind::AfterTool, DisconnectKind::PartialAfterTool] {
        let provider = DisconnectProvider::new(1, kind);
        let result = AgentLoop::new(pump_config())
            .run(
                &provider,
                &NoOpExecutor,
                vec![Message::user("hello")],
                vec![],
            )
            .await
            .expect("errors carried by loop result");
        assert!(result.llm_error.is_some());
        assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn stream_connection_retry_backoff_honors_cancellation() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cfg = install_retry(2, 30_000, 30_000);
    let provider = DisconnectProvider::new(usize::MAX, DisconnectKind::BodyReset);
    let token = tokio_util::sync::CancellationToken::new();
    let (tx, mut rx) = mpsc::channel(1024);
    let watch_token = token.clone();
    let watcher = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if matches!(event, AgentLoopEvent::Progress { stage, .. } if stage == "model_retrying")
            {
                watch_token.cancel();
                break;
            }
        }
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        AgentLoop::new(pump_config()).run_with_events(
            &provider,
            &NoOpExecutor,
            vec![Message::user("hello")],
            vec![],
            Some(tx),
            Some(token),
        ),
    )
    .await
    .expect("cancel without waiting for backoff")
    .expect("cancelled run");
    watcher.await.expect("watcher");
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn completed_pump_call_reports_model_and_usage() {
    let _lock = STREAM_RETRY_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let provider = FlakyPartialProvider::new(0, "done");
    let (tx, rx) = mpsc::channel(1024);
    AgentLoop::new(pump_config())
        .run_with_events(
            &provider,
            &NoOpExecutor,
            vec![Message::user("hello")],
            vec![],
            Some(tx),
            None,
        )
        .await
        .expect("turn completes");

    let calls: Vec<_> = collect_events(rx)
        .await
        .into_iter()
        .filter_map(|event| match event {
            AgentLoopEvent::Debug(crate::events::DebugEvent::LlmCall {
                provider,
                model,
                input_tokens,
                output_tokens,
                ..
            }) => Some((provider, model, input_tokens, output_tokens)),
            _ => None,
        })
        .collect();
    assert_eq!(
        calls,
        [(
            "flaky-partial-test".to_string(),
            "claude-test-model".to_string(),
            1,
            1
        )]
    );
}
