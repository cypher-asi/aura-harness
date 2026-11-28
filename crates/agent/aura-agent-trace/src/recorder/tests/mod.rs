//! Tests of the step builder and the recorder.

use super::*;
use aura_agent::DebugEvent;
use aura_config::HarnessParams;
use aura_core_types::trace::{
    Actor, DecisionSource, HarnessAction, HarnessDecision, ModelAction, ModelExchange, Observation,
    StepAction, TokenUsage, ToolCallAction, ToolIntent,
};
use serde_json::json;

use crate::records::{CallRow, RouteRow};

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

pub(crate) fn exchange(iteration: u32, calls: &[(&str, &str, Value)]) -> AgentLoopEvent {
    AgentLoopEvent::Debug(DebugEvent::ModelExchange {
        timestamp: now(),
        exchange: Box::new(ModelExchange {
            iteration,
            model: "m".into(),
            observation: None,
            action: ModelAction {
                tool_calls: calls
                    .iter()
                    .map(|(id, name, input)| ToolCallAction {
                        tool_use_id: (*id).into(),
                        name: (*name).into(),
                        input: input.clone(),
                        intent: ToolIntent::classify(name, input),
                    })
                    .collect(),
                stop_reason: "tool_use".into(),
                ..ModelAction::default()
            },
            usage: TokenUsage {
                input: 100,
                output: 10,
                cache_write: 0,
                cache_read: 50,
            },
            latency_ms: 7,
            provider_request_id: None,
        }),
        task_id: None,
    })
}

mod builder;
mod gate;
mod recorder;
