use super::*;

#[tokio::test]
async fn each_call_reports_its_result_when_it_finishes() {
    let executor = GatedExecutor {
        release: Notify::new(),
    };
    let config = AgentLoopConfig::for_agent("claude-test-model");
    let stream = mk_stream(vec![
        mk_call("toolu_slow", "slow"),
        mk_call("toolu_fast", "fast"),
        ResponseEvent::Completed {
            stop_reason: None,
            end_turn: Some(false),
            usage: Usage::new(1, 1),
        },
    ]);
    let mut state = crate::agent_loop::LoopState::new_for_tests(&config, Vec::new());
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let responded = std::sync::atomic::AtomicBool::new(false);
    let on_response = |_: &ModelResponse| {
        responded.store(true, std::sync::atomic::Ordering::SeqCst);
    };

    let drive = drive_stream(
        StreamPumpCtx {
            event_tx: Some(&tx),
            on_response: Some(&on_response),
            ..test_ctx(&config, &executor)
        },
        stream,
        &mut state,
        "test-model",
    );
    let watch = async {
        let mut results = Vec::new();
        while let Some(event) = rx.recv().await {
            if let AgentLoopEvent::ToolResult { tool_use_id, .. } = event {
                assert!(
                    responded.load(std::sync::atomic::Ordering::SeqCst),
                    "the model call is reported before its tools finish"
                );
                if tool_use_id == "toolu_fast" {
                    executor.release.notify_one();
                }
                results.push(tool_use_id);
                if results.len() == 2 {
                    return results;
                }
            }
        }
        results
    };
    let (outcome, results) =
        tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(drive, watch) })
            .await
            .expect("the fast call must not wait for the slow one");

    assert_eq!(results, ["toolu_fast", "toolu_slow"]);
    match outcome {
        StreamPumpOutcome::Completed { tool_results, .. } => {
            let ids: Vec<_> = tool_results.iter().map(|(c, _)| c.id.as_str()).collect();
            assert_eq!(
                ids,
                ["toolu_slow", "toolu_fast"],
                "the model still gets them in order"
            );
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
    assert_eq!(state.reported_tool_results.len(), 2);
}

#[tokio::test]
async fn cancelling_keeps_the_real_result_of_a_call_that_already_ran() {
    let executor = GatedExecutor {
        release: Notify::new(),
    };
    let config = AgentLoopConfig::for_agent("claude-test-model");
    let stream = mk_stream(vec![
        mk_call("toolu_slow", "slow"),
        mk_call("toolu_fast", "fast"),
        ResponseEvent::Completed {
            stop_reason: None,
            end_turn: Some(false),
            usage: Usage::new(1, 1),
        },
    ]);
    let mut state = crate::agent_loop::LoopState::new_for_tests(&config, Vec::new());
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let cancel = CancellationToken::new();

    let drive = drive_stream(
        StreamPumpCtx {
            event_tx: Some(&tx),
            cancellation_token: Some(&cancel),
            ..test_ctx(&config, &executor)
        },
        stream,
        &mut state,
        "test-model",
    );
    let cancel_after_fast = async {
        while let Some(event) = rx.recv().await {
            if matches!(event, AgentLoopEvent::ToolResult { ref tool_use_id, .. } if tool_use_id == "toolu_fast")
            {
                cancel.cancel();
                return;
            }
        }
    };
    let (outcome, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(drive, cancel_after_fast)
    })
    .await
    .expect("cancel must end the drain");

    let StreamPumpOutcome::Completed { tool_results, .. } = outcome else {
        panic!("expected Completed with cancelled results, got {outcome:?}");
    };
    let results: Vec<(&str, &str)> = tool_results
        .iter()
        .map(|(c, r)| (c.id.as_str(), r.content.as_str()))
        .collect();
    assert_eq!(results[0].0, "toolu_slow");
    assert!(results[0].1.starts_with("[CANCELLED]"), "{results:?}");
    assert_eq!(
        results[1],
        ("toolu_fast", "done"),
        "the finished call keeps its result"
    );
}

#[tokio::test(start_paused = true)]
async fn cancelling_waits_for_an_edit_already_in_flight() {
    let executor = SlowEditExecutor {
        edit_delay: Some(Duration::from_millis(200)),
    };
    let results = cancel_with_edit_in_flight(&executor).await;
    assert_eq!(
        results[0],
        ("toolu_edit".to_string(), "edited".to_string()),
        "an edit that lands after the cancel must report that it landed"
    );
}

#[tokio::test(start_paused = true)]
async fn cancelling_an_unfinished_edit_says_it_may_have_applied() {
    let executor = SlowEditExecutor { edit_delay: None };
    let results = cancel_with_edit_in_flight(&executor).await;
    assert_eq!(results[0].0, "toolu_edit");
    assert!(
        results[0].1.starts_with("[CANCELLED]") && results[0].1.contains("re-read the file"),
        "{results:?}"
    );
    assert_eq!(results[1], ("toolu_fast".to_string(), "done".to_string()));
}
