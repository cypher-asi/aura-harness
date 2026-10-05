//! Output allowance and termination regression tests.
use super::{AgentLoop, AgentLoopConfig, LoopState};
use crate::agent_runner::{configure_loop_config, AgentRunnerConfig};
use crate::turn_config::TaskComplexity;
use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};
use aura_model_reasoner::{
    ContentBlock, Message, ModelProvider, ModelRequest, ModelResponse, ProviderTrace,
    ReasonerError, Role, StopReason, ThinkingEffort, ToolDefinition, Usage,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

fn request_caps(config: &AgentLoopConfig, n: usize) -> Vec<u32> {
    let mut state = LoopState::new(config, vec![Message::user("implement")]);
    (0..n)
        .map(|iteration| {
            state.begin_iteration(config, iteration);
            state
                .build_request(config, &[], iteration)
                .unwrap()
                .max_tokens
                .get()
        })
        .collect()
}

#[test]
fn task_wire_caps_fit_edits_and_respect_the_ceiling() {
    let runner = AgentRunnerConfig::for_agent("claude-opus-4-7");
    for complexity in [
        TaskComplexity::Simple,
        TaskComplexity::Standard,
        TaskComplexity::Complex,
    ] {
        let config = configure_loop_config(complexity, &runner, 1, "system".into());
        let caps = request_caps(&config, 4);
        eprintln!(
            "{complexity:?}: nominal={} emitted={caps:?}",
            config.max_tokens
        );
        assert_eq!(caps, vec![config.max_tokens; 4]);
    }
}

#[test]
fn chat_preserves_selected_effort_and_output_allowance() {
    let config = AgentLoopConfig {
        user_thinking_effort: Some(ThinkingEffort::XHigh),
        ..AgentLoopConfig::for_agent("test-model")
    };
    assert_eq!(request_caps(&config, 5), vec![16384; 5]);
    let mut state = LoopState::new(&config, vec![]);
    state.had_any_file_write = true;
    for iteration in 0..5 {
        state.begin_iteration(&config, iteration);
        assert_eq!(
            state
                .build_request(&config, &[], iteration)
                .unwrap()
                .thinking_effort,
            Some(ThinkingEffort::XHigh)
        );
    }
}

#[test]
fn explicit_maximum_is_never_exceeded() {
    let config = AgentLoopConfig {
        max_tokens: 1024,
        ..AgentLoopConfig::for_agent("test-model")
    };
    let caps = request_caps(&config, 4);
    eprintln!("explicit max=1024 emitted={caps:?}");
    assert_eq!(caps, vec![1024; 4]);
}

#[derive(Default)]
struct CountingExecutor(AtomicUsize);
#[async_trait::async_trait]
impl AgentToolExecutor for CountingExecutor {
    async fn execute(&self, calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        self.0.fetch_add(calls.len(), Ordering::SeqCst);
        calls
            .iter()
            .map(|c| ToolCallResult::success(c.id.clone(), "committed once"))
            .collect()
    }
}

struct CapturingProvider {
    requests: Mutex<Vec<ModelRequest>>,
    with_tool: bool,
}
#[async_trait::async_trait]
impl ModelProvider for CapturingProvider {
    fn name(&self) -> &'static str {
        "audit-probe"
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        let mut requests = self.requests.lock().unwrap();
        let call = requests.len();
        requests.push(request);
        let (reason, content) = if call == 0 {
            if self.with_tool {
                (
                    StopReason::MaxTokens,
                    vec![ContentBlock::ToolUse {
                        id: "once".into(),
                        name: "write_file".into(),
                        input: serde_json::json!({"path":"probe.txt", "content":"valid completed tool input"}),
                    }],
                )
            } else {
                (
                    StopReason::MaxTokens,
                    vec![ContentBlock::text("Partial answer cut mid-sentence")],
                )
            }
        } else {
            (StopReason::EndTurn, vec![ContentBlock::text("Finished")])
        };
        Ok(ModelResponse::new(
            reason,
            Message::new(Role::Assistant, content),
            Usage::new(100, 800),
            ProviderTrace::new("probe", 0),
        ))
    }
    async fn health_check(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn text_truncation_continues_without_losing_partial_output() {
    let provider = CapturingProvider {
        requests: Mutex::new(vec![]),
        with_tool: false,
    };
    let result = AgentLoop::new(AgentLoopConfig::for_agent("test-model"))
        .run(
            &provider,
            &CountingExecutor::default(),
            vec![Message::user("explain")],
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    assert_eq!(result.iterations, 2);
    assert!(
        result
            .total_text
            .contains("Partial answer cut mid-sentence")
            && result.total_text.contains("Finished")
    );
    assert!(!result.output_truncated);
    assert!(result.llm_error.is_none() && !result.stalled && !result.timed_out);
}

#[tokio::test]
async fn max_tokens_preserves_completed_tools_without_replaying() {
    let provider = CapturingProvider {
        requests: Mutex::new(vec![]),
        with_tool: true,
    };
    let executor = CountingExecutor::default();
    let config = configure_loop_config(
        TaskComplexity::Standard,
        &AgentRunnerConfig::for_agent("test-model"),
        1,
        "system".into(),
    );
    let result = AgentLoop::new(config)
        .run(
            &provider,
            &executor,
            vec![Message::user("write")],
            vec![ToolDefinition::new(
                "write_file",
                "write",
                serde_json::json!({"type":"object"}),
            )],
        )
        .await
        .unwrap();
    let requests = provider.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|r| r.max_tokens.get())
            .collect::<Vec<_>>(),
        vec![16384, 16384]
    );
    assert_eq!(executor.0.load(Ordering::SeqCst), 1);
    assert!(requests[1]
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .any(|b| matches!(
            b,
            ContentBlock::ToolResult {
                is_error: false,
                ..
            }
        )));
    assert_eq!(result.iterations, 2);
    assert!(result.llm_error.is_none());
}

#[tokio::test]
async fn tracked_task_rejects_text_without_accepted_task_done() {
    use crate::agent_runner::{AgentRunner, AgenticTaskParams, TaskTrackingConfig};
    use aura_context_prompts::{ProjectInfo, SessionInfo, SpecInfo, TaskInfo};
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().to_string_lossy().into_owned();
    let project = ProjectInfo {
        project_id: None,
        name: "probe",
        description: "",
        folder_path: &folder,
        build_command: None,
        test_command: None,
    };
    let spec = SpecInfo {
        title: "probe",
        markdown_contents: "Write the requested file",
    };
    let task = TaskInfo {
        title: "Create implementation",
        description: "Implement the requested file",
        execution_notes: "",
        files_changed: &[],
    };
    let session = SessionInfo {
        summary_of_previous_context: "",
    };
    let params = AgenticTaskParams {
        project: &project,
        spec: &spec,
        task: &task,
        session: &session,
        work_log: &[],
        completed_deps: &[],
        workspace_map: "",
        codebase_snapshot: "",
        type_defs_context: "",
        dep_api_context: "",
        member_count: 1,
        tools: vec![],
        attempt: 0,
        agent: None,
    };
    let executor = std::sync::Arc::new(CountingExecutor::default());
    let tracking = TaskTrackingConfig {
        inner_executor: executor.clone(),
        project_folder: folder.clone(),
        build_command: None,
        test_command: None,
        early_test_oracle: None,
    };
    let provider = CapturingProvider {
        requests: Mutex::new(vec![]),
        with_tool: false,
    };
    let result = AgentRunner::new(AgentRunnerConfig::for_agent("test-model"))
        .execute_task_tracked(&provider, tracking, &params, None, None)
        .await;
    assert!(
        result.is_err(),
        "text or a passing build is not a task_done handshake"
    );
    assert_eq!(executor.0.load(Ordering::SeqCst), 0);
    let provider = aura_model_reasoner::MockProvider::new().with_response(
        aura_model_reasoner::MockResponse::tool_use(
            "done",
            "task_done",
            serde_json::json!({"no_changes_needed":true, "notes":"Already correct"}),
        ),
    );
    let tracking = TaskTrackingConfig {
        inner_executor: executor,
        project_folder: folder.clone(),
        build_command: None,
        test_command: None,
        early_test_oracle: None,
    };
    let completed = AgentRunner::new(AgentRunnerConfig::for_agent("test-model"))
        .execute_task_tracked(&provider, tracking, &params, None, None)
        .await;
    assert!(
        completed.is_ok(),
        "accepted no-change task_done should succeed: {completed:?}"
    );
}

#[tokio::test]
async fn repeated_truncation_is_bounded_and_not_reported_as_success() {
    use aura_model_reasoner::{MockProvider, MockResponse};
    let provider = MockProvider::new()
        .with_response(MockResponse::text("one").with_stop_reason(StopReason::MaxTokens))
        .with_response(MockResponse::text("two").with_stop_reason(StopReason::MaxTokens))
        .with_response(MockResponse::text("three").with_stop_reason(StopReason::MaxTokens))
        .with_response(MockResponse::text("must not be called"));
    let (sender, mut receiver) = tokio::sync::mpsc::channel(100);
    let result = AgentLoop::new(AgentLoopConfig::for_agent("test-model"))
        .run_with_events(
            &provider,
            &CountingExecutor::default(),
            vec![Message::user("explain")],
            vec![],
            Some(sender),
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.iterations, 3);
    assert!(result.output_truncated);
    assert!(!result.total_text.contains("must not be called"));
    let mut limit_error = false;
    while let Ok(event) = receiver.try_recv() {
        limit_error |= matches!(event, crate::events::AgentLoopEvent::Error { code, recoverable: false, .. } if code == "output_limit");
    }
    assert!(limit_error, "bounded failure must be visible to the user");
}

#[test]
fn explicit_initial_budget_cannot_exceed_response_ceiling() {
    let config = AgentLoopConfig {
        max_tokens: 1024,
        thinking_budget: Some(64_000),
        ..AgentLoopConfig::for_agent("test-model")
    };
    assert_eq!(request_caps(&config, 8), vec![1024; 8]);
}
