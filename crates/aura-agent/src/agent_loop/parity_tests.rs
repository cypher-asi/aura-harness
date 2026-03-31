//! Parity tests verifying parallel execution, timeouts, and policy enforcement
//! in the `KernelToolExecutor` ↔ `AgentLoop` stack.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aura_core::{Action, ActionKind, AgentId, Effect, ToolCall, ToolResult};
use aura_kernel::{
    ExecuteContext, Executor, ExecutorError, ExecutorRouter, PermissionLevel, Policy, PolicyConfig,
};
use aura_reasoner::{
    ContentBlock, Message, MockProvider, MockResponse, StopReason, ToolDefinition, Usage,
};

use crate::agent_loop::{AgentLoop, AgentLoopConfig};
use crate::kernel_executor::KernelToolExecutor;
use crate::types::{AgentToolExecutor, ToolCallInfo};

// ---------------------------------------------------------------------------
// Stub kernel-level executor
// ---------------------------------------------------------------------------

/// Returns a canned `ToolResult::success` for every delegate action.
struct StubExecutor;

#[async_trait]
impl Executor for StubExecutor {
    async fn execute(
        &self,
        _ctx: &ExecuteContext,
        action: &Action,
    ) -> Result<Effect, ExecutorError> {
        let tool_call: ToolCall = serde_json::from_slice(&action.payload)
            .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;
        let result = ToolResult::success(&tool_call.tool, format!("ok:{}", tool_call.tool));
        let payload = serde_json::to_vec(&result)
            .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;
        Ok(Effect::committed_agreement(action.action_id, payload))
    }

    fn can_handle(&self, action: &Action) -> bool {
        action.kind == ActionKind::Delegate
    }

    fn name(&self) -> &'static str {
        "stub"
    }
}

/// Sleeps for a configurable duration before returning, used to trigger timeouts.
struct SlowExecutor {
    delay: Duration,
}

#[async_trait]
impl Executor for SlowExecutor {
    async fn execute(
        &self,
        _ctx: &ExecuteContext,
        action: &Action,
    ) -> Result<Effect, ExecutorError> {
        tokio::time::sleep(self.delay).await;
        let tool_call: ToolCall = serde_json::from_slice(&action.payload)
            .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;
        let result = ToolResult::success(&tool_call.tool, "slow-done");
        let payload = serde_json::to_vec(&result)
            .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;
        Ok(Effect::committed_agreement(action.action_id, payload))
    }

    fn can_handle(&self, action: &Action) -> bool {
        action.kind == ActionKind::Delegate
    }

    fn name(&self) -> &'static str {
        "slow"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn stub_router() -> ExecutorRouter {
    ExecutorRouter::with_executors(vec![Arc::new(StubExecutor)])
}

fn make_tool_call_info(id: &str, name: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: id.to_string(),
        name: name.to_string(),
        input: serde_json::json!({}),
    }
}

fn two_tool_use_response() -> MockResponse {
    MockResponse {
        stop_reason: StopReason::ToolUse,
        content: vec![
            ContentBlock::tool_use("t1", "read_file", serde_json::json!({"path": "a.txt"})),
            ContentBlock::tool_use("t2", "read_file", serde_json::json!({"path": "b.txt"})),
        ],
        usage: Usage::new(100, 50),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn parallel_read_tools_execute_concurrently() {
    let agent_id = AgentId::generate();
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();

    let executor =
        KernelToolExecutor::new(stub_router(), agent_id, workspace).with_parallel();

    let provider = MockProvider::new()
        .with_response(two_tool_use_response())
        .with_response(MockResponse::text("done"));

    let config = AgentLoopConfig {
        system_prompt: "test".to_string(),
        ..AgentLoopConfig::default()
    };
    let agent = AgentLoop::new(config);
    let messages = vec![Message::user("go")];
    let tools = vec![ToolDefinition::new(
        "read_file",
        "Read a file",
        serde_json::json!({"type": "object"}),
    )];

    let result = agent.run(&provider, &executor, messages, tools).await.unwrap();

    assert_eq!(result.iterations, 2, "should run tool-use + final turn");
    assert!(result.total_text.contains("done"));
}

#[tokio::test]
async fn parallel_tools_preserve_result_order() {
    let agent_id = AgentId::generate();
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();

    let executor =
        KernelToolExecutor::new(stub_router(), agent_id, workspace).with_parallel();

    let calls = vec![
        make_tool_call_info("t1", "read_file"),
        make_tool_call_info("t2", "list_files"),
    ];

    let results = executor.execute(&calls).await;

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].tool_use_id, "t1");
    assert_eq!(results[1].tool_use_id, "t2");
    assert!(!results[0].is_error);
    assert!(!results[1].is_error);
    assert!(results[0].content.contains("read_file"));
    assert!(results[1].content.contains("list_files"));
}

#[tokio::test]
async fn tool_timeout_returns_error() {
    let agent_id = AgentId::generate();
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();

    let router = ExecutorRouter::with_executors(vec![Arc::new(SlowExecutor {
        delay: Duration::from_secs(5),
    })]);

    let executor = KernelToolExecutor::new(router, agent_id, workspace)
        .with_timeout(Duration::from_millis(50));

    let calls = vec![make_tool_call_info("t1", "read_file")];
    let results = executor.execute(&calls).await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(
        results[0].content.contains("timed out"),
        "expected timeout message, got: {}",
        results[0].content,
    );
}

#[tokio::test]
async fn policy_deny_returns_error_result() {
    let agent_id = AgentId::generate();
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();

    let policy_config =
        PolicyConfig::default().with_tool_permission("delete_file", PermissionLevel::Deny);
    let policy = Policy::new(policy_config);

    let executor =
        KernelToolExecutor::new(stub_router(), agent_id, workspace).with_policy(policy);

    let calls = vec![make_tool_call_info("t1", "delete_file")];
    let results = executor.execute(&calls).await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(
        results[0].content.contains("not allowed by policy"),
        "expected policy denial, got: {}",
        results[0].content,
    );
}

#[tokio::test]
async fn policy_deny_does_not_block_allowed_tools() {
    let agent_id = AgentId::generate();
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();

    let policy_config =
        PolicyConfig::default().with_tool_permission("delete_file", PermissionLevel::Deny);
    let policy = Policy::new(policy_config);

    let executor =
        KernelToolExecutor::new(stub_router(), agent_id, workspace).with_policy(policy);

    let calls = vec![make_tool_call_info("t1", "read_file")];
    let results = executor.execute(&calls).await;

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].is_error,
        "read_file should be allowed, got error: {}",
        results[0].content,
    );
    assert!(results[0].content.contains("read_file"));
}
