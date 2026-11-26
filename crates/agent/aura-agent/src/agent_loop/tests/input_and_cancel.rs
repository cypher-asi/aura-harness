use super::*;

use crate::session::input_queue::InputQueue;
use crate::session::SessionId;
use crate::{AgentRunnerHandle, UserInput};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

// ------------------------------------------------------------------
// Layer E.2 — input queue (mid-task user steering)
// ------------------------------------------------------------------

/// User message that [`InjectMessageOnFirstCall`] queues mid-turn.
const FOLLOW_UP: &str = "follow-up: please summarise what you just did";

fn queue_test_agent() -> AgentLoop {
    AgentLoop::new(AgentLoopConfig {
        system_prompt: "test agent".to_string(),
        ..AgentLoopConfig::for_agent("claude-test-model")
    })
}

/// Non-streaming run options wired to `handle` and its cancellation token.
fn queue_run_options<'a>(
    cancel: &CancellationToken,
    handle: &'a AgentRunnerHandle,
) -> crate::agent_loop::RunOptions<'a> {
    crate::agent_loop::RunOptions {
        event_tx: None,
        cancellation_token: Some(cancel.clone()),
        handle: Some(handle),
    }
}

struct InjectMessageOnFirstCall {
    queue: Arc<InputQueue>,
    call_count: AtomicUsize,
}

#[async_trait::async_trait]
impl ModelProvider for InjectMessageOnFirstCall {
    fn name(&self) -> &'static str {
        "inject-message-on-first-call"
    }

    async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);
        let text = if n == 0 {
            self.queue
                .push(UserInput::Message(FOLLOW_UP.into()))
                .await
                .expect("queue is open");
            "First call done."
        } else {
            "Second call after queued input."
        };
        Ok(ModelResponse::new(
            StopReason::EndTurn,
            Message::assistant(text),
            Usage::new(80, 20),
            ProviderTrace::new("e2-mock", 0),
        ))
    }

    async fn health_check(&self) -> bool {
        true
    }
}

/// User input arrives mid-turn. Expectation:
///
///   - Sampling 1 runs the original `user → "kick off"` message,
///     model emits `EndTurn`. From inside `provider.complete()` we
///     push a [`UserInput::Message`] onto the queue so that — at the
///     bottom of the turn loop — `has_pending() == true` and the
///     turn loop re-enters instead of breaking out cleanly.
///   - The next iteration drains the queued message into
///     `state.messages` (via [`crate::helpers::append_warning`], which
///     merges into the trailing user block to preserve Anthropic
///     `tool_use` / `tool_result` adjacency), then runs sampling 2.
///   - Sampling 2 emits a second `EndTurn` referencing the queued
///     content. After that, `has_pending() == false` and the turn
///     loop terminates cleanly.
///
/// Asserts the total iteration count, the queued message body
/// landing in the final message history, and that the provider only
/// got two calls (the loop did NOT spin extra rounds beyond the one
/// the queue triggered).
///
/// Uses a scripted-fakes provider (no `tokio::time::sleep` or wall
/// clock ordering) per Rule 7.3 — every event ordering assertion
/// here is provider-driven, not time-driven.
#[tokio::test]
async fn pending_input_extends_turn_loop() {
    let cancel = CancellationToken::new();
    let handle = AgentRunnerHandle::new(SessionId::new_v4(), cancel.clone());
    let provider = InjectMessageOnFirstCall {
        queue: handle.queue(),
        call_count: AtomicUsize::new(0),
    };
    let executor = MockExecutor { results: vec![] };

    let agent = queue_test_agent();

    let result = agent
        .run_with_session(
            &provider,
            &executor,
            vec![Message::user("kick off")],
            vec![],
            queue_run_options(&cancel, &handle),
        )
        .await
        .expect("E.2 happy path must succeed");

    assert_eq!(
        provider.call_count.load(Ordering::SeqCst),
        2,
        "queued input must trigger exactly one extra sampling request"
    );
    assert_eq!(
        result.iterations, 2,
        "iteration counter monotonic across the queue-extended sampling"
    );
    assert!(
        result
            .total_text
            .contains("Second call after queued input."),
        "the second EndTurn message must surface on the result"
    );
    assert!(
        result
            .messages
            .iter()
            .any(|msg| msg.text_content().contains(FOLLOW_UP)),
        "queued user-input body must have been appended to the message history before sampling 2"
    );
    assert!(
        !cancel.is_cancelled(),
        "non-cancel user input must NOT fire the cancellation token"
    );
    assert!(
        !handle.has_pending(),
        "queue must be empty after the loop drains the message"
    );
}

struct PanicOnExecute;
#[async_trait::async_trait]
impl AgentToolExecutor for PanicOnExecute {
    async fn execute(&self, _tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        panic!("tool executor must NOT run after a Cancel push");
    }
}

struct CancelOnFirstCall {
    queue: Arc<InputQueue>,
    call_count: AtomicUsize,
}

#[async_trait::async_trait]
impl ModelProvider for CancelOnFirstCall {
    fn name(&self) -> &'static str {
        "cancel-on-first-call"
    }

    async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            n, 0,
            "provider must NOT be called a second time after cancel"
        );
        // ToolUse (without follow-through) would normally make the
        // turn loop dispatch tools. We fire UserInput::Cancel
        // *before* returning so the post-call `is_cancelled` check
        // in `run_sampling_request` short-circuits the dispatch
        // (and the PanicOnExecute executor never gets a chance to
        // run). This pins the atomic-cancel invariant from Rule
        // 6.3: the partial response is never appended together
        // with a half-executed tool batch.
        self.queue
            .push(UserInput::Cancel)
            .await
            .expect("queue is open");
        Ok(ModelResponse::new(
            StopReason::ToolUse,
            Message {
                role: aura_model::Role::Assistant,
                content: vec![ContentBlock::tool_use(
                    "toolu_cancelled",
                    "read_file",
                    serde_json::json!({"path": "ignored.rs"}),
                )],
            },
            Usage::new(50, 10),
            ProviderTrace::new("e2-mock", 0),
        ))
    }

    async fn health_check(&self) -> bool {
        true
    }
}

/// [`UserInput::Cancel`] unwinds the active turn via the shared
/// cancellation token, without leaving the message history in a
/// half-written state.
///
///   - Sampling 1 runs the original user prompt, model emits a
///     `ToolUse` (so `needs_follow_up = true`). From inside
///     `provider.complete()` we push [`UserInput::Cancel`] onto the
///     queue, which fires the cancellation token before the inner
///     loop's post-sampling `is_cancelled` check.
///   - The post-call `is_cancelled` check inside
///     [`crate::agent_loop::sampling::run_sampling_request`] short-
///     circuits the tool dispatch and returns
///     `broke_for_error = true`. The turn loop unwinds without
///     entering another sampling.
///
/// Asserts the cancellation token is observed (cancelled), the tool
/// dispatch never fires (the executor would have panicked otherwise),
/// and the provider is not called a second time. Uses a scripted-
/// fakes provider so the ordering is fully deterministic (Rule 7.3).
#[tokio::test]
async fn cancel_unwinds_active_turn() {
    let cancel = CancellationToken::new();
    let handle = AgentRunnerHandle::new(SessionId::new_v4(), cancel.clone());
    let provider = CancelOnFirstCall {
        queue: handle.queue(),
        call_count: AtomicUsize::new(0),
    };
    let executor = PanicOnExecute;

    let agent = queue_test_agent();

    let result = agent
        .run_with_session(
            &provider,
            &executor,
            vec![Message::user("start a long task")],
            vec![ToolDefinition::new(
                "read_file",
                "Read a file",
                serde_json::json!({"type": "object"}),
            )],
            queue_run_options(&cancel, &handle),
        )
        .await
        .expect("cancel must unwind cleanly, not propagate as Err");

    assert!(
        cancel.is_cancelled(),
        "UserInput::Cancel must fire the shared cancellation token"
    );
    assert_eq!(
        provider.call_count.load(Ordering::SeqCst),
        1,
        "no second sampling request after cancel"
    );
    assert!(
        result.iterations <= 1,
        "loop must not pay for sampling beyond the cancel boundary"
    );
}

/// Closed-queue path: once [`AgentRunnerHandle::close`] runs, any
/// subsequent `send_user_input` surfaces a typed
/// [`crate::AgentError::InputQueueClosed`] with the originating
/// session id. Mirrors Rules 4.1 / 4.3: no silent drops on session
/// teardown.
#[tokio::test]
async fn send_user_input_after_close_returns_input_queue_closed() {
    use crate::session::SessionId;
    use crate::{AgentRunnerHandle, UserInput};
    use tokio_util::sync::CancellationToken;

    let cancel = CancellationToken::new();
    let session_id = SessionId::new_v4();
    let handle = AgentRunnerHandle::new(session_id, cancel);
    handle.close();
    let err = handle
        .send_user_input(UserInput::Message("late arrival".into()))
        .await
        .expect_err("send must fail after close");
    match err {
        crate::AgentError::InputQueueClosed { session_id: got } => {
            assert_eq!(
                got, session_id,
                "error must carry the originating session id"
            );
        }
        other => panic!("expected InputQueueClosed, got {other:?}"),
    }
}
