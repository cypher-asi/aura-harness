use super::*;

#[tokio::test]
async fn malformed_tool_json_is_rejected_without_execution() {
    let executor = CountingExecutor::default();
    let config = AgentLoopConfig::for_agent("test-model");
    let frames = vec![
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::ToolUse {
                id: "partial".into(),
                name: "write_file".into(),
            },
        }),
        Ok(StreamEvent::InputJsonDelta {
            partial_json: "{\"path\":\"file\",\"content\":\"unfinished".into(),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::MaxTokens),
            output_tokens: 100,
            input_tokens: None,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
        Ok(StreamEvent::MessageStop),
    ];
    let stream = response_stream_from_event_stream(Box::pin(futures_util::stream::iter(frames)));
    let (outcome, _) = drive(&config, &executor, stream).await;
    let StreamPumpOutcome::Completed {
        response,
        tool_results,
    } = outcome
    else {
        panic!("unexpected outcome: {outcome:?}");
    };
    assert_eq!(response.stop_reason, StopReason::MaxTokens);
    assert_eq!(tool_results.len(), 1);
    assert!(tool_results[0].1.is_error);
    assert_eq!(tool_results[0].1.tool_use_id, "partial");
    assert!(tool_results[0].1.content.contains("No tool was executed"));
    assert!(executor.invocations.lock().await.is_empty());
    assert_eq!(
        executor.presets(),
        vec![("partial".to_string(), "invalid_tool_use")],
        "an invalid tool use must still reach the executor to be recorded"
    );
}

#[tokio::test]
async fn missing_terminal_event_is_not_clean_completion() {
    let executor = CountingExecutor::default();
    let config = AgentLoopConfig::for_agent("test-model");
    let (outcome, _) = drive(&config, &executor, mk_stream(vec![])).await;
    assert!(!matches!(outcome, StreamPumpOutcome::Completed { .. }));
}

#[tokio::test]
async fn pump_drains_in_fifo_submission_order() {
    let executor = CountingExecutor::default();
    let config = AgentLoopConfig::for_agent("claude-test-model");
    let stream = mk_stream(vec![
        mk_call("toolu_a", "read_file"),
        mk_call("toolu_b", "read_file"),
        mk_call("toolu_c", "read_file"),
        completed(false),
    ]);
    let (outcome, _) = drive(&config, &executor, stream).await;
    match outcome {
        StreamPumpOutcome::Completed { tool_results, .. } => {
            let ids: Vec<_> = tool_results.iter().map(|(c, _)| c.id.clone()).collect();
            assert_eq!(ids, vec!["toolu_a", "toolu_b", "toolu_c"]);
        }
        _ => panic!("expected Completed outcome"),
    }
}

#[tokio::test]
async fn pump_cancellation_yields_atomic_no_write() {
    let executor = CountingExecutor::default();
    let config = AgentLoopConfig {
        stream_event_timeout: Duration::from_secs(30),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let cancel = CancellationToken::new();
    cancel.cancel();
    let stream: ResponseEventStream = Box::pin(futures_util::stream::pending());
    let mut state = crate::agent_loop::LoopState::new_for_tests(&config, Vec::new());

    let outcome = drive_stream(
        StreamPumpCtx {
            cancellation_token: Some(&cancel),
            ..test_ctx(&config, &executor)
        },
        stream,
        &mut state,
        "test-model",
    )
    .await;
    assert!(matches!(outcome, StreamPumpOutcome::Cancelled));
    assert!(state.messages.is_empty(), "no state mutation on cancel");
}

#[tokio::test]
async fn pump_per_outputitemdone_input_drain() {
    let executor = CountingExecutor::default();
    let config = AgentLoopConfig::for_agent("claude-test-model");
    let cancel = CancellationToken::new();
    let queue = InputQueue::new(SessionId::new_v4(), cancel.clone());
    // Drive: one tool call, then user types something between
    // tool calls, then another tool call, then Completed.
    // The drain happens after the first tool call's
    // OutputItemDone, so the second iteration's apply-step
    // should already see the pushed input.
    queue
        .push(crate::session::UserInput::Message("queued-message".into()))
        .await
        .expect("pump test push: queue is drop-on-cancel only");
    let _ = cancel; // keep alive
    let stream = mk_stream(vec![
        mk_call("toolu_a", "read_file"),
        mk_call("toolu_b", "read_file"),
        completed(false),
    ]);
    let mut state = crate::agent_loop::LoopState::new_for_tests(&config, Vec::new());

    let outcome = drive_stream(
        StreamPumpCtx {
            input_queue: Some(&queue),
            ..test_ctx(&config, &executor)
        },
        stream,
        &mut state,
        "test-model",
    )
    .await;
    assert!(matches!(outcome, StreamPumpOutcome::Completed { .. }));
    // The queued message should have been drained mid-pump and
    // appended to state.messages.
    assert!(
        state
            .messages
            .iter()
            .any(|m| m.content.iter().any(|b| matches!(
                b,
                aura_model::ContentBlock::Text { text } if text.contains("queued-message")
            ))),
        "drained user input must be appended to messages mid-pump"
    );
}

#[tokio::test(start_paused = true)]
async fn pump_stream_event_timeout_surfaces_typed_error() {
    let executor = CountingExecutor::default();
    let config = AgentLoopConfig {
        stream_event_timeout: Duration::from_secs(5),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let stream: ResponseEventStream = Box::pin(futures_util::stream::pending());
    let (outcome, _) = drive(&config, &executor, stream).await;
    assert!(matches!(
        outcome,
        StreamPumpOutcome::Error(AgentError::StreamTimeout { .. })
    ));
}

/// 5 gaps of 3s = 15s of "thinking" — 3x the 5s boundary — but no
/// single inter-frame gap exceeds it: four keepalives, then `Completed`.
fn keepalive_stream() -> ResponseEventStream {
    let gap = Duration::from_secs(3);
    Box::pin(futures_util::stream::unfold(0usize, move |i| async move {
        if i < 4 {
            tokio::time::sleep(gap).await;
            Some((
                Ok::<_, aura_model::StreamError>(ResponseEvent::Keepalive(StreamPhase::Thinking)),
                i + 1,
            ))
        } else if i == 4 {
            tokio::time::sleep(gap).await;
            Some((Ok(completed(true)), i + 1))
        } else {
            None
        }
    }))
}

/// A stream that emits a keepalive (ping/delta) every `< stream_event_timeout`
/// must NOT trip `StreamTimeout`, even when the total span before the
/// first completed block far exceeds the boundary. This is the
/// liveness contract: pings/deltas reset the per-event window so a
/// long extended-thinking block stays alive.
#[tokio::test(start_paused = true)]
async fn pump_keepalive_resets_liveness_timeout() {
    let config = AgentLoopConfig {
        stream_event_timeout: Duration::from_secs(5),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    // Advance the paused clock in 3s steps; each step releases one
    // inter-frame sleep without ever letting a 5s timeout window
    // elapse. Yield first so the driver re-arms its next
    // `timeout(5s, stream.next())` before we advance.
    let clock = ClockSteps {
        rounds: 10,
        yields: 16,
        step: Duration::from_secs(3),
    };
    let executor = CountingExecutor::default();
    let driver = spawn_and_advance(config, executor, keepalive_stream(), clock).await;

    let res = tokio::time::timeout(Duration::from_secs(600), driver).await;
    assert!(res.is_ok(), "pump should complete, not hang/timeout");
    let outcome = res.unwrap().expect("driver join");
    assert!(
        matches!(outcome, StreamPumpOutcome::Completed { .. }),
        "keepalive-fed stream must complete, not trip StreamTimeout"
    );
}
