//! Phase 4 keystone — single [`ModelTransport`] trait around the
//! streaming pump ([`super::stream_pump::run_stream_pump`]) sampling
//! path.
//!
//! # Why
//!
//! Pre-Phase-4 the agent loop carried two near-duplicate sampling
//! entry points in [`super::sampling`]: `run_sampling_request`
//! (buffered) and `run_sampling_request_streaming` (pump). Roughly
//! 80% of the body was shared — cancellation probe,
//! `accumulate_response`, `emit_iteration_complete`,
//! `dispatch_stop_reason` — but the two duplicated retry, error
//! mapping, and tool-batch handoff. The duplication was the single
//! biggest source of dual-path parity bugs.
//!
//! Phase 4 folded the model-sampling step into a trait so the
//! enclosing sampling driver runs the cancellation / accumulate /
//! iteration_complete / dispatch tail exactly once regardless of
//! transport. Phase 7 then deleted the legacy [`BufferedTransport`]
//! after parity tests proved the pump was production-ready and no
//! caller flipped `use_stream_pump` to `false` outside tests. The
//! trait is kept as a stable seam for future transports (an offline
//! cassette transport for replay, for example) without re-opening
//! the dual-path complexity.
//!
//! The [`TransportOutcome`] variants carry the only thing the
//! transport legitimately produces (an optional pre-executed tool
//! batch) and downstream `process_tool_results` consumes it via
//! [`super::tool_pipeline::ToolBatch::PreExecuted`].
//!
//! # SamplingCtx vs ToolEffectCtx
//!
//! [`SamplingCtx`] bundles the per-sample arguments — agent,
//! provider, executor, tools, event channel, cancellation token,
//! input queue, mutable loop state, and the freshly-built request.
//! The transport takes it by value (move) so the `&mut LoopState`
//! borrow inside is single-use per sample.
//!
//! [`super::tool_pipeline::ToolEffectCtx`] is a *separate*, much
//! smaller bundle threaded through `process_tool_results` (executor,
//! event_tx, cancellation_token). The two contexts intentionally do
//! not share a struct: sample-time needs the request and provider,
//! tool-effect-time needs neither, and packing them together would
//! force the post-sample dispatch path to carry dead fields.
//!
//! # Cancellation contract
//!
//! [`TransportOutcome::Cancelled`] surfaces the
//! `StreamPumpOutcome::Cancelled` short-circuit as a single
//! "no llm_error, broke_for_error = true" signal so the sampling
//! driver can short-circuit without applying a synthetic
//! `llm_error` string to the result.
//!
//! Mid-tool cancellation inside the pump still folds `[CANCELLED]`
//! tool_results into a `Streamed` outcome with the synthetic
//! `stop_loop = true` markers (see
//! `super::stream_pump::driver::cancelled_outcome`) so the
//! Anthropic `tool_use ↔ tool_result` adjacency contract stays
//! intact through `process_tool_results`. Returning `Cancelled`
//! here is reserved for the "no tool_use blocks emitted yet" arms.

use aura_reasoner::{ModelProvider, ModelRequest, ModelResponse, ToolDefinition};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

use crate::events::AgentLoopEvent;
use crate::session::input_queue::InputQueue;
use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};

use super::iteration::LlmCallError;
use super::stream_pump::{run_stream_pump, StreamPumpOutcome};
use super::{AgentLoop, LoopState};

/// Bundle of borrowed per-sample dependencies handed to
/// [`ModelTransport::sample`] each turn.
///
/// All fields except [`Self::request`] are borrowed; the request is
/// owned because it is built once per sample (see
/// [`super::state::LoopState::build_request`]) and the transport
/// consumes it (the pump uses `.clone()` internally for retries, so
/// it is `Clone`).
///
/// `&mut LoopState` is stored so the pump path can update
/// [`super::cache::ToolResultCache`] / `state.messages` / the
/// repeated-read tracker mid-stream; the buffered path takes it but
/// does not need to mutate `state` from inside `sample` — the
/// outer `run_sampling_request` body owns post-response mutations.
pub(crate) struct SamplingCtx<'a> {
    pub(crate) agent: &'a AgentLoop,
    pub(crate) provider: &'a dyn ModelProvider,
    pub(crate) executor: &'a dyn AgentToolExecutor,
    /// Tool catalog handed to the model. Kept on the ctx for the
    /// future `cassette` transport (the pump consumes the catalog
    /// through the pre-built `request`).
    #[allow(dead_code)]
    pub(crate) tools: &'a [ToolDefinition],
    pub(crate) event_tx: Option<&'a Sender<AgentLoopEvent>>,
    pub(crate) cancellation_token: Option<&'a CancellationToken>,
    pub(crate) input_queue: Option<&'a InputQueue>,
    pub(crate) state: &'a mut LoopState,
    pub(crate) request: ModelRequest,
    /// 0-based sampling iteration index. Kept on the ctx for
    /// transports that need to drive iteration-keyed telemetry; the
    /// pump consumes the counter through `state.result.iterations`.
    #[allow(dead_code)]
    pub(crate) iteration: usize,
}

/// One sampling round-trip outcome.
///
/// `Streamed` carries the response plus the FIFO-ordered
/// pre-executed tool batch the pump already ran inside the streaming
/// driver. `process_tool_results` consumes it as
/// `ToolBatch::PreExecuted` — `track_tool_effects` / `auto-build` /
/// message-push then run on top.
///
/// `Cancelled` is the "no llm_error, just break" short-circuit. It
/// fires when the cancellation token observed during sampling did
/// NOT have any in-flight tool_use blocks to repair (otherwise the
/// pump path folds `[CANCELLED]` tool_results into `Streamed`).
pub(crate) enum TransportOutcome {
    Streamed {
        response: ModelResponse,
        pre_executed: Vec<(ToolCallInfo, ToolCallResult)>,
    },
    Cancelled,
}

/// The keystone trait: one method, one outcome enum.
///
/// Phase 7 collapsed the implementation set from two
/// (`BufferedTransport` + `PumpTransport`) to one ([`PumpTransport`])
/// after parity tests confirmed the pump as the production default.
/// The trait stays `pub(crate)` per Rule 3.1 so the seam is preserved
/// for future transports (offline cassette replay, mock transports
/// for property tests) without re-opening the dual-path complexity.
#[async_trait::async_trait]
pub(crate) trait ModelTransport: Send + Sync {
    /// Drive one sampling request to terminal completion.
    ///
    /// Returns [`TransportOutcome::Streamed`] for the pump path
    /// (with the pre-executed tool batch), or
    /// [`TransportOutcome::Cancelled`] when the cancellation token
    /// fired before any tool_use blocks were emitted.
    ///
    /// # Errors
    ///
    /// Returns the structured [`LlmCallError`] for fatal model
    /// errors (rate-limit, prompt-too-long, insufficient credits,
    /// transport blowups).
    async fn sample(&self, ctx: SamplingCtx<'_>) -> Result<TransportOutcome, LlmCallError>;
}

/// Streaming pump transport: wraps the
/// [`run_stream_pump`] entry point.
///
/// Drives `provider.complete_response_stream(...)` with per-event
/// timeout, overlaps tool execution at `OutputItemDone` boundaries
/// via [`futures_util::stream::FuturesOrdered`], and returns the
/// pre-executed tool batch in [`TransportOutcome::Streamed`].
/// Mid-tool cancellation folds `[CANCELLED]` tool_results into the
/// `pre_executed` vec (see `driver::cancelled_outcome`) so the
/// downstream `process_tool_results` step closes the Anthropic
/// adjacency contract before the loop breaks.
pub(crate) struct PumpTransport;

#[async_trait::async_trait]
impl ModelTransport for PumpTransport {
    async fn sample(&self, ctx: SamplingCtx<'_>) -> Result<TransportOutcome, LlmCallError> {
        let SamplingCtx {
            agent,
            provider,
            executor,
            event_tx,
            cancellation_token,
            input_queue,
            state,
            request,
            ..
        } = ctx;

        let outcome = run_stream_pump(
            &agent.config,
            provider,
            executor,
            request,
            cancellation_token,
            input_queue,
            event_tx,
            state,
        )
        .await;

        match outcome {
            StreamPumpOutcome::Completed {
                response,
                tool_results,
            } => Ok(TransportOutcome::Streamed {
                response,
                pre_executed: tool_results,
            }),
            StreamPumpOutcome::Cancelled => Ok(TransportOutcome::Cancelled),
            StreamPumpOutcome::Error(err) => {
                let llm_err = match err {
                    crate::AgentError::Reason(inner) => LlmCallError::from_reasoner_error(&inner),
                    other => LlmCallError::Fatal(other.to_string()),
                };
                Err(llm_err)
            }
            StreamPumpOutcome::AbortedWithPartial { .. } => Err(LlmCallError::Fatal(
                "stream pump returned an unretried partial tool-use abort".to_string(),
            )),
        }
    }
}

/// Hand back the singleton pump transport reference.
///
/// Returns a `&'static dyn ModelTransport` so the sampling driver
/// can call `sample` without per-turn allocation. Phase 7 collapsed
/// the previous `select_transport(config)` toggle into this
/// no-argument helper after `use_stream_pump` and the buffered
/// transport were removed.
pub(crate) fn active_transport() -> &'static dyn ModelTransport {
    &PumpTransport
}
