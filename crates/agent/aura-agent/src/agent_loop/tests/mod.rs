//! Core agent loop tests: config defaults, simple runs, error handling, max-tokens.

use aura_model::{
    ContentBlock, Message, MockProvider, MockResponse, ModelProvider, ModelRequest,
    ModelRequestKind, ModelResponse, ProviderTrace, ReasonerError, StopReason, ToolDefinition,
    Usage,
};

use std::sync::atomic::{AtomicUsize, Ordering};

use std::sync::Mutex;

use tokio::sync::mpsc;

use super::{AgentLoop, AgentLoopConfig};

use crate::events::AgentLoopEvent;

use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};

struct MockExecutor {
    results: Vec<ToolCallResult>,
}

#[async_trait::async_trait]
impl AgentToolExecutor for MockExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        tool_calls
            .iter()
            .zip(self.results.iter())
            .map(|(tc, r)| ToolCallResult {
                tool_use_id: tc.id.clone(),
                ..r.clone()
            })
            .collect()
    }
}

struct SummaryThenFinalProvider {
    call_count: AtomicUsize,
    request_kinds: Mutex<Vec<Option<ModelRequestKind>>>,
}

#[async_trait::async_trait]
impl ModelProvider for SummaryThenFinalProvider {
    fn name(&self) -> &'static str {
        "summary-then-final"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        self.request_kinds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.metadata.kind);
        let call = self.call_count.fetch_add(1, Ordering::SeqCst);
        let text = if call == 0 {
            "Summary: earlier turns explored large implementation details."
        } else {
            "Done after summary compaction."
        };
        Ok(ModelResponse::new(
            StopReason::EndTurn,
            Message::assistant(text),
            Usage::new(100, 20),
            ProviderTrace::new("summary-mock", 0),
        ))
    }

    async fn health_check(&self) -> bool {
        true
    }
}

mod compaction;
mod end_turn;
mod input_and_cancel;
mod iteration_effort;
mod run;
mod turn_control;
