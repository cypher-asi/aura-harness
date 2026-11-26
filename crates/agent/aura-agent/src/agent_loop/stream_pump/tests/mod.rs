//! Pump-scope unit tests.
//!
//! Note: the bulk of the E.3 mandatory tests live in
//! `crate::agent_loop::stream_pump_tests` so they can use scripted
//! fake providers and `start_paused = true` time control without
//! pulling the full sampling driver into scope.

use std::time::Duration;

use async_trait::async_trait;
use aura_model::{
    response_stream_from_event_stream, ContentBlock, Message, ModelResponse, OutputItem,
    ProviderTrace, ResponseEvent, ResponseEventStream, Role, StopReason, StreamContentType,
    StreamEvent, StreamEventStream, StreamPhase, Usage,
};

use tokio::sync::Notify;

use tokio_util::sync::CancellationToken;

use crate::events::AgentLoopEvent;

use crate::session::input_queue::InputQueue;

use crate::session::SessionId;

use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};

use crate::AgentError;

use super::driver::drive_stream;

use super::AgentLoopConfig;

use super::{StreamPumpCtx, StreamPumpOutcome};

/// Phase 8 test helper: build a [`StreamPumpCtx`] for the
/// driver-scope unit tests without forcing every call site to
/// repeat the five-field literal. The four optional fields default
/// to `None`; positional `cancellation_token` / `input_queue` /
/// `event_tx` may be supplied via the explicit constructors below.
fn test_ctx<'a>(
    config: &'a AgentLoopConfig,
    executor: &'a dyn AgentToolExecutor,
) -> StreamPumpCtx<'a> {
    StreamPumpCtx {
        config,
        executor,
        cancellation_token: None,
        input_queue: None,
        event_tx: None,
        on_response: None,
    }
}

#[derive(Default)]
struct CountingExecutor {
    invocations: tokio::sync::Mutex<Vec<ToolCallInfo>>,
    presets: std::sync::Mutex<Vec<(String, &'static str)>>,
}

#[async_trait]
impl AgentToolExecutor for CountingExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        let mut guard = self.invocations.lock().await;
        for call in tool_calls {
            guard.push(call.clone());
        }
        tool_calls
            .iter()
            .map(|tc| ToolCallResult::success(tc.id.clone(), format!("ok:{}", tc.name)))
            .collect()
    }

    fn spawn_preset<'a>(
        &'a self,
        call: ToolCallInfo,
        source: &'static str,
        result: ToolCallResult,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolCallResult> + Send + 'a>> {
        self.presets.lock().unwrap().push((call.id, source));
        Box::pin(std::future::ready(result))
    }
}

impl CountingExecutor {
    fn presets(&self) -> Vec<(String, &'static str)> {
        self.presets.lock().unwrap().clone()
    }
}

fn mk_call(id: &str, name: &str) -> ResponseEvent {
    ResponseEvent::OutputItemDone(OutputItem::ToolUse {
        id: id.into(),
        name: name.into(),
        input: serde_json::json!({}),
    })
}

fn mk_stream(events: Vec<ResponseEvent>) -> ResponseEventStream {
    Box::pin(futures_util::stream::iter(
        events.into_iter().map(Ok::<_, aura_model::StreamError>),
    ))
}

/// The terminal `Completed` event with unit usage and no stop reason.
fn completed(end_turn: bool) -> ResponseEvent {
    ResponseEvent::Completed {
        stop_reason: None,
        end_turn: Some(end_turn),
        usage: Usage::new(1, 1),
    }
}

/// A response calling tool `name` (call `id`), then `fast` (call
/// `toolu_fast`), not ending the turn.
fn first_then_fast(id: &str, name: &str) -> ResponseEventStream {
    mk_stream(vec![
        mk_call(id, name),
        mk_call("toolu_fast", "fast"),
        completed(false),
    ])
}

/// Cancels `cancel` once the `fast` call reports its result on `rx`.
async fn cancel_when_fast_reports(
    rx: &mut tokio::sync::mpsc::Receiver<AgentLoopEvent>,
    cancel: &CancellationToken,
) {
    while let Some(event) = rx.recv().await {
        if matches!(event, AgentLoopEvent::ToolResult { ref tool_use_id, .. } if tool_use_id == "toolu_fast")
        {
            cancel.cancel();
            return;
        }
    }
}

/// Drives `stream` through the pump with a bare context and returns the
/// outcome with the state it left behind.
async fn drive(
    config: &AgentLoopConfig,
    executor: &dyn AgentToolExecutor,
    stream: ResponseEventStream,
) -> (StreamPumpOutcome, super::super::LoopState) {
    let mut state = super::super::LoopState::new_for_tests(config, Vec::new());
    let outcome = drive_stream(test_ctx(config, executor), stream, &mut state, "test-model").await;
    (outcome, state)
}

/// Drives `stream` with an event channel plumbed in, asserts the pump
/// completed, and returns every event it emitted.
async fn drive_collecting_events(
    config: &AgentLoopConfig,
    executor: &dyn AgentToolExecutor,
    stream: ResponseEventStream,
) -> Vec<AgentLoopEvent> {
    let mut state = super::super::LoopState::new_for_tests(config, Vec::new());
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let outcome = drive_stream(
        StreamPumpCtx {
            event_tx: Some(&tx),
            ..test_ctx(config, executor)
        },
        stream,
        &mut state,
        "test-model",
    )
    .await;
    assert!(matches!(outcome, StreamPumpOutcome::Completed { .. }));
    drop(tx);
    let mut events: Vec<AgentLoopEvent> = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    events
}

/// Spawns the pump over `stream`, then advances the paused clock by
/// `step` `rounds` times, yielding `yields` times before each advance so
/// the driver re-arms its timers first.
async fn spawn_and_advance(
    config: AgentLoopConfig,
    executor: impl AgentToolExecutor + 'static,
    stream: ResponseEventStream,
    clock: ClockSteps,
) -> tokio::task::JoinHandle<StreamPumpOutcome> {
    let driver = tokio::spawn(async move {
        let mut state = super::super::LoopState::new_for_tests(&config, Vec::new());
        drive_stream(
            test_ctx(&config, &executor),
            stream,
            &mut state,
            "test-model",
        )
        .await
    });
    for _ in 0..clock.rounds {
        for _ in 0..clock.yields {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(clock.step).await;
    }
    driver
}

/// How [`spawn_and_advance`] moves the paused clock.
struct ClockSteps {
    rounds: usize,
    yields: usize,
    step: Duration,
}

/// Runs every call at once, but holds `slow` until `release` fires, the
/// way a call waiting on an approval holds up nothing else.
struct GatedExecutor {
    release: Notify,
}

#[async_trait]
impl AgentToolExecutor for GatedExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        let mut results = Vec::new();
        for call in tool_calls {
            if call.name == "slow" {
                self.release.notified().await;
            }
            results.push(ToolCallResult::success(call.id.clone(), "done"));
        }
        results
    }
}

/// `edit_file` takes `edit_delay` to land, or never lands when `None`;
/// `fast` returns at once.
struct SlowEditExecutor {
    edit_delay: Option<Duration>,
}

#[async_trait]
impl AgentToolExecutor for SlowEditExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        let mut results = Vec::new();
        for call in tool_calls {
            if call.name == "edit_file" {
                match self.edit_delay {
                    Some(delay) => tokio::time::sleep(delay).await,
                    None => std::future::pending::<()>().await,
                }
                results.push(ToolCallResult::success(call.id.clone(), "edited"));
            } else {
                results.push(ToolCallResult::success(call.id.clone(), "done"));
            }
        }
        results
    }
}

/// Cancels once `fast` reports, while `edit_file` is still in flight.
async fn cancel_with_edit_in_flight(executor: &SlowEditExecutor) -> Vec<(String, String)> {
    let config = AgentLoopConfig::for_agent("claude-test-model");
    let stream = first_then_fast("toolu_edit", "edit_file");
    let mut state = super::super::LoopState::new_for_tests(&config, Vec::new());
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let cancel = CancellationToken::new();
    let drive = drive_stream(
        StreamPumpCtx {
            event_tx: Some(&tx),
            cancellation_token: Some(&cancel),
            ..test_ctx(&config, executor)
        },
        stream,
        &mut state,
        "test-model",
    );
    let (outcome, ()) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(drive, cancel_when_fast_reports(&mut rx, &cancel))
    })
    .await
    .expect("cancel must end the drain");
    let StreamPumpOutcome::Completed { tool_results, .. } = outcome else {
        panic!("expected Completed with cancelled results, got {outcome:?}");
    };
    tool_results
        .into_iter()
        .map(|(c, r)| (c.id, r.content))
        .collect()
}

mod concurrency;
mod deltas;
mod lifecycle;
mod results;
mod tool_hooks;
