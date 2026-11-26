//! Cutover tests: AgentLoop equivalents of every former TurnProcessor test.
//!
//! These tests verify that AgentLoop covers the same behavioral surface as
//! the removed TurnProcessor, ensuring no coverage was lost during migration.

use aura_model::{ContentBlock, Message, MockProvider, MockResponse, ToolDefinition};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{AgentLoop, AgentLoopConfig};
use crate::events::TurnEvent;
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

fn default_config() -> AgentLoopConfig {
    AgentLoopConfig {
        system_prompt: "cutover test agent".to_string(),
        ..AgentLoopConfig::for_agent("claude-test-model")
    }
}

fn read_file_tool() -> ToolDefinition {
    ToolDefinition::new(
        "read_file",
        "Read a file",
        serde_json::json!({"type": "object"}),
    )
}

mod integration;
mod loop_api;
mod loop_flow;
