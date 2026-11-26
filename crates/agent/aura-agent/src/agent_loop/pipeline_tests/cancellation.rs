use super::*;

/// Stop pressed while a tool is in flight: the agent loop must observe
/// cancellation via the `tokio::select!` in `process_tool_results`,
/// synthesize a `[CANCELLED]` tool_result with `stop_loop: true`, and
/// break the loop on the first iteration's `check_termination_conditions`
/// — not after the long-running executor naturally returns. Pinned
/// because the pre-fix behaviour was "warn-and-continue": stop ran on
/// the harness side but `executor.execute(...).await` had no cancel
/// observation, so the loop kept the agent alive (and any spawned
/// child processes) for as long as the tool took to finish.
#[tokio::test]
async fn pipeline_cancellation_mid_tool_execution_aborts_loop() {
    use std::time::Duration;
    use tokio::time::timeout;

    let provider = MockProvider::new().with_response(MockResponse::tool_use(
        "tc_slow",
        "read_file",
        serde_json::json!({"path": "huge.txt"}),
    ));

    let started = Arc::new(Notify::new());
    let unblock = Arc::new(Notify::new());
    let executor = BlockingExecutor {
        started: Arc::clone(&started),
        unblock: Arc::clone(&unblock),
    };

    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("read huge.txt")];
    let tools = vec![read_file_tool()];
    let cancel = CancellationToken::new();
    let cancel_for_run = cancel.clone();

    let run = tokio::spawn(async move {
        agent
            .run_with_events(
                &provider,
                &executor,
                messages,
                tools,
                None,
                Some(cancel_for_run),
            )
            .await
    });

    timeout(Duration::from_secs(2), started.notified())
        .await
        .expect("executor.execute must be reached before cancellation fires");

    cancel.cancel();

    let result = timeout(Duration::from_secs(2), run)
        .await
        .expect("loop must observe cancellation and terminate without unblock")
        .expect("loop task must not panic")
        .expect("loop must return Ok even when cancelled mid-tool");

    let cancelled_tool_result = has_cancelled_tool_result(&result.messages);
    assert!(
        cancelled_tool_result,
        "mid-tool cancellation must synthesize a [CANCELLED] tool_result so the \
         Anthropic tool_use<->tool_result adjacency contract stays intact"
    );
    assert_eq!(
        result.iterations, 1,
        "the loop must break on the same iteration as the cancellation; pre-fix \
         this iterated until the executor naturally returned"
    );
}

use std::sync::atomic::AtomicBool;

struct PoisonExecutor {
    called: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl AgentToolExecutor for PoisonExecutor {
    async fn execute(&self, _tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        self.called.store(true, Ordering::SeqCst);
        panic!("executor.execute must NOT run when cancellation is observed pre-dispatch");
    }
}

/// Cancels the supplied token the first time `complete_streaming`
/// is invoked. Simulates the "Stop pressed during the model's
/// stream" race that motivated the post-`call_model` cancellation
/// check in `run_inner`.
struct CancellingProvider {
    inner: StreamingMockProvider,
    cancel: CancellationToken,
}

#[async_trait::async_trait]
impl ModelProvider for CancellingProvider {
    fn name(&self) -> &'static str {
        "cancelling_mock"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        self.inner.complete(request).await
    }

    async fn complete_streaming(
        &self,
        request: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        let stream = self.inner.complete_streaming(request).await?;
        self.cancel.cancel();
        Ok(stream)
    }

    async fn health_check(&self) -> bool {
        true
    }
}

/// Stop pressed between `call_model` returning and tool dispatch: the
/// agent loop's post-streaming cancellation check must short-circuit
/// `dispatch_stop_reason` so no fresh tool batch starts after the user
/// pressed Stop. Pre-fix the loop happily dispatched the new batch and
/// only noticed cancellation at the top of the NEXT iteration.
#[tokio::test]
async fn pipeline_cancellation_after_call_model_skips_tool_dispatch() {
    let inner = MockProvider::new().with_response(MockResponse::tool_use(
        "tc_after",
        "read_file",
        serde_json::json!({"path": "a.txt"}),
    ));
    let cancel = CancellationToken::new();
    let provider = CancellingProvider {
        inner: StreamingMockProvider { inner },
        cancel: cancel.clone(),
    };

    let called = Arc::new(AtomicBool::new(false));
    let executor = PoisonExecutor {
        called: Arc::clone(&called),
    };

    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("read a.txt")];
    let tools = vec![read_file_tool()];
    // `event_tx = Some(_)` is required to take the streaming path
    // through `complete_with_streaming` — the non-streaming
    // `provider.complete(...)` branch never observes the cancellation
    // triggered by the provider during `complete_streaming`.
    let (event_tx, _event_rx) = mpsc::channel(1024);

    let result = agent
        .run_with_events(
            &provider,
            &executor,
            messages,
            tools,
            Some(event_tx),
            Some(cancel),
        )
        .await
        .expect("loop must return Ok when cancellation is observed post-streaming");

    assert!(
        !called.load(Ordering::SeqCst),
        "executor.execute must not run after cancellation observed post-streaming"
    );
    // Either the streaming layer's own cancellation check (`DrainOutcome::Cancelled`)
    // or `run_inner`'s post-`call_model` check (the new defense-in-depth guard
    // added alongside this test) is allowed to win — both result in "tools
    // never dispatched after Stop". `iterations == 0` happens when streaming
    // wins (cancellation breaks the drain loop before `accumulate_response` /
    // `state.result.iterations = iteration + 1` run); `iterations == 1` happens
    // when streaming completes synchronously and the post-`call_model` check
    // breaks the loop after the iteration counter advances.
    assert!(
        result.iterations <= 1,
        "the loop must break on or before the first iteration; got {iters}",
        iters = result.iterations
    );
}

/// Whether any message carries a `[CANCELLED]` text tool_result.
fn has_cancelled_tool_result(messages: &[Message]) -> bool {
    messages.iter().any(|msg| {
        msg.content.iter().any(|block| {
            matches!(
                block,
                ContentBlock::ToolResult {
                    content: ToolResultContent::Text(text),
                    ..
                } if text.contains("[CANCELLED]")
            )
        })
    })
}
