use super::*;

/// Sleeps 5s per call before reporting `ok`.
#[derive(Default)]
struct SleepyExecutor;

#[async_trait]
impl AgentToolExecutor for SleepyExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        let mut out = Vec::new();
        for tc in tool_calls {
            tokio::time::sleep(Duration::from_secs(5)).await;
            out.push(ToolCallResult::success(tc.id.clone(), "ok"));
        }
        out
    }
}

/// Confirm the pump uses true per-tool concurrency: when three
/// tools each sleep for 5s using the paused tokio clock, the
/// FIFO drains after a single 5s `advance` rather than the
/// 15s a sequential dispatcher would need.
#[tokio::test(start_paused = true)]
async fn pump_overlaps_concurrent_tools() {
    let config = AgentLoopConfig::for_agent("claude-test-model");
    let stream = mk_stream(vec![
        mk_call("toolu_a", "t"),
        mk_call("toolu_b", "t"),
        mk_call("toolu_c", "t"),
        completed(false),
    ]);
    // Let the spawned task progress to the await on the first
    // sleep. We can't deterministically know when, so yield a
    // few times to give it scheduler attention before advancing
    // the clock.
    // Single 5s advance must complete ALL three sleeps if the
    // pump is overlapping. With a 15s advance the test would
    // pass even for a sequential executor, so a 5s window is
    // the discriminating signal.
    let clock = ClockSteps {
        rounds: 1,
        yields: 16,
        step: Duration::from_secs(5),
    };
    let driver = spawn_and_advance(config, SleepyExecutor, stream, clock).await;
    // With overlap, the pump returns after the single advance (all 3
    // sleeps completed in parallel). The wait is bounded by a
    // generous timeout so a sequential executor would surface
    // as a wait-timeout panic.
    let res = tokio::time::timeout(Duration::from_secs(120), driver).await;
    assert!(
        res.is_ok(),
        "pump should complete after single 5s advance when tools overlap"
    );
    let outcome = res.unwrap().expect("driver join");
    match outcome {
        StreamPumpOutcome::Completed { tool_results, .. } => {
            assert_eq!(tool_results.len(), 3);
            let ids: Vec<_> = tool_results.iter().map(|(c, _)| c.id.clone()).collect();
            assert_eq!(ids, vec!["toolu_a", "toolu_b", "toolu_c"]);
        }
        _ => panic!("expected Completed"),
    }
}

/// Hangs (600s, way past the 10s per-tool timeout) on calls named
/// `hang`; every other call reports `ok` at once.
#[derive(Default)]
struct PartiallyHungExecutor;

#[async_trait]
impl AgentToolExecutor for PartiallyHungExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        let mut out = Vec::new();
        for tc in tool_calls {
            if tc.name == "hang" {
                // Sleep way past the 10s per-tool timeout.
                tokio::time::sleep(Duration::from_secs(600)).await;
                out.push(ToolCallResult::success(tc.id.clone(), "unreachable"));
            } else {
                out.push(ToolCallResult::success(tc.id.clone(), "ok"));
            }
        }
        out
    }
}

/// Confirm a hung tool that exceeds `per_tool_timeout` resolves
/// to a synthetic error result without poisoning the FIFO. The
/// other tools in the batch still produce their normal results.
#[tokio::test(start_paused = true)]
async fn pump_per_tool_timeout_does_not_poison_fifo() {
    let config = AgentLoopConfig {
        per_tool_timeout: Duration::from_secs(10),
        stream_event_timeout: Duration::from_secs(120),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let stream = mk_stream(vec![
        mk_call("toolu_a", "ok"),
        mk_call("toolu_b", "hang"),
        mk_call("toolu_c", "ok"),
        completed(false),
    ]);
    let clock = ClockSteps {
        rounds: 1,
        yields: 32,
        step: Duration::from_secs(11),
    };
    let driver = spawn_and_advance(config, PartiallyHungExecutor, stream, clock).await;
    let outcome = tokio::time::timeout(Duration::from_secs(120), driver)
        .await
        .expect("driver did not complete after timeout window")
        .expect("driver join");
    match outcome {
        StreamPumpOutcome::Completed { tool_results, .. } => {
            assert_eq!(tool_results.len(), 3);
            assert!(!tool_results[0].1.is_error);
            assert!(tool_results[1].1.is_error, "hung tool should error");
            assert!(
                tool_results[1].1.content.contains("timed out"),
                "hung tool should mention timeout"
            );
            assert!(
                !tool_results[2].1.is_error,
                "subsequent tools must still produce their normal result"
            );
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}
