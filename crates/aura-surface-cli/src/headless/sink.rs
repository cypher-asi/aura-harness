//! Headless counterpart of the terminal's turn forwarder: turns
//! [`AgentLoopEvent`]s into `stream-json` lines, counts model calls and
//! cost, stops the turn when a run limit is reached, and records the
//! events to an eval run's trace.

use super::output::{ExitReason, Output, StreamEvent};
use crate::event_loop::hud::call_stats;
use crate::session_checkpoint::SessionUsage;
use async_trait::async_trait;
use aura_agent::{
    map_agent_loop_event, AgentLoopEvent, DebugEvent, TurnEventSink, PROGRESS_CONTEXT_SUMMARIZED,
    PROGRESS_TODOS_UPDATED,
};
use aura_agent_trace::RunRecorder;
use aura_model_reasoner::Usage;
use aura_tools::todo::TodoItem;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;

/// Run-wide limits the sink enforces as model calls report in.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CallLimits {
    pub max_calls: Option<u32>,
    pub max_cost_usd: Option<f64>,
}

/// What the sink saw during one turn.
#[derive(Debug, Default)]
pub(crate) struct TurnTally {
    /// Model calls and tokens of the turn
    pub usage: SessionUsage,
    /// The limit that stopped the turn, if one did
    pub limit_hit: Option<ExitReason>,
}

pub(crate) struct HeadlessSink {
    output: Output,
    /// Usage of the run's earlier turns, for run-wide limits
    before: SessionUsage,
    limits: CallLimits,
    stop: CancellationToken,
    context_limit: Option<u64>,
    /// Inputs of finished tool calls, until their result arrives
    inputs: HashMap<String, serde_json::Value>,
    tally: TurnTally,
    trace: Option<Arc<RunRecorder>>,
}

impl HeadlessSink {
    pub(crate) fn new(
        output: Output,
        before: SessionUsage,
        limits: CallLimits,
        stop: CancellationToken,
        context_limit: Option<u64>,
    ) -> Self {
        Self {
            output,
            before,
            limits,
            stop,
            context_limit,
            inputs: HashMap::new(),
            tally: TurnTally::default(),
            trace: None,
        }
    }

    /// Also record every event to `trace`.
    pub(crate) fn with_trace(mut self, trace: Option<Arc<RunRecorder>>) -> Self {
        self.trace = trace;
        self
    }

    /// Drain `rx` until the turn drops its sender.
    pub(crate) async fn forward(mut self, mut rx: mpsc::Receiver<AgentLoopEvent>) -> TurnTally {
        while let Some(event) = rx.recv().await {
            if let Some(trace) = &self.trace {
                trace.on_event(&event);
            }
            map_agent_loop_event(event, &mut self).await;
        }
        self.tally
    }

    fn enforce_limits(&mut self) {
        if self.tally.limit_hit.is_some() {
            return;
        }
        let mut run = self.before;
        run.add(&self.tally.usage);
        let hit = if self.limits.max_calls.is_some_and(|max| run.calls >= max) {
            Some(ExitReason::MaxTurns)
        } else if self
            .limits
            .max_cost_usd
            .is_some_and(|max| run.priced && run.cost_usd > max)
        {
            Some(ExitReason::MaxCost)
        } else {
            None
        };
        if let Some(reason) = hit {
            debug!(
                ?reason,
                calls = run.calls,
                cost_usd = run.cost_usd,
                "Run limit reached"
            );
            self.tally.limit_hit = Some(reason);
            // The loop checks the token before its next model call, so a
            // call already answered still has its tools run and recorded.
            self.stop.cancel();
        }
    }
}

#[async_trait]
impl TurnEventSink for HeadlessSink {
    async fn on_text_delta(&mut self, text: String) {
        self.output.event(&StreamEvent::TextDelta { text: &text });
    }

    async fn on_thinking_delta(&mut self, text: String) {
        self.output
            .event(&StreamEvent::ThinkingDelta { text: &text });
    }

    async fn on_tool_start(&mut self, id: String, name: String) {
        self.output.event(&StreamEvent::ToolUse {
            id: &id,
            name: &name,
        });
    }

    async fn on_tool_call_completed(
        &mut self,
        tool_use_id: String,
        _tool_name: String,
        input: serde_json::Value,
        _is_error: bool,
    ) {
        self.inputs.insert(tool_use_id, input);
    }

    async fn on_tool_result(
        &mut self,
        tool_use_id: String,
        tool_name: String,
        content: String,
        is_error: bool,
        _image: Option<aura_core_types::ToolResultImage>,
    ) {
        let input = self
            .inputs
            .remove(&tool_use_id)
            .unwrap_or(serde_json::Value::Null);
        self.output.event(&StreamEvent::ToolResult {
            id: &tool_use_id,
            name: &tool_name,
            input: &input,
            content: &content,
            is_error,
        });
    }

    async fn on_tool_call_failed(
        &mut self,
        tool_use_id: String,
        tool_name: String,
        reason: String,
    ) {
        self.output.event(&StreamEvent::ToolResult {
            id: &tool_use_id,
            name: &tool_name,
            input: &serde_json::Value::Null,
            content: &reason,
            is_error: true,
        });
    }

    async fn on_stream_reset(
        &mut self,
        reason: String,
        _text_bytes: usize,
        _thinking_bytes: usize,
    ) {
        self.output
            .event(&StreamEvent::StreamReset { reason: &reason });
    }

    async fn on_warning(&mut self, message: String) {
        self.output
            .event(&StreamEvent::Warning { message: &message });
    }

    async fn on_error(&mut self, _code: String, message: String, _recoverable: bool) {
        self.output.event(&StreamEvent::Error { message: &message });
    }

    async fn on_debug(&mut self, event: DebugEvent) {
        let DebugEvent::LlmCall {
            model,
            input_tokens,
            output_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
            ..
        } = event
        else {
            return;
        };
        let usage = Usage {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
        };
        let stats = call_stats(&model, &usage, self.context_limit);
        self.tally.usage.add(&SessionUsage {
            calls: 1,
            input_tokens: stats.input_tokens,
            cache_read_tokens: stats.cache_read_tokens,
            cache_write_tokens: stats.cache_write_tokens,
            output_tokens: stats.output_tokens,
            cost_usd: stats.cost_usd.unwrap_or(0.0),
            priced: stats.cost_usd.is_some(),
            context_tokens: stats.prompt_tokens(),
        });
        self.output.event(&StreamEvent::Usage {
            model: &model,
            input_tokens: stats.input_tokens,
            cache_read_tokens: stats.cache_read_tokens,
            cache_write_tokens: stats.cache_write_tokens,
            output_tokens: stats.output_tokens,
            cost_usd: stats.cost_usd,
        });
        self.enforce_limits();
    }

    async fn on_progress(
        &mut self,
        stage: String,
        _tool_name: Option<String>,
        _elapsed_ms: Option<u64>,
        message: Option<String>,
    ) {
        let Some(message) = message else {
            return;
        };
        match stage.as_str() {
            PROGRESS_TODOS_UPDATED => match serde_json::from_str::<Vec<TodoItem>>(&message) {
                Ok(todos) => self.output.event(&StreamEvent::Todos { todos: &todos }),
                Err(e) => debug!(error = %e, "Unreadable todo list update"),
            },
            PROGRESS_CONTEXT_SUMMARIZED => {
                self.output
                    .event(&StreamEvent::Compacted { message: &message });
            }
            _ => {}
        }
    }
}
