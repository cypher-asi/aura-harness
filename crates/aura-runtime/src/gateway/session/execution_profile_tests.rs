//! Execution effort and terminal UI status regressions.
use super::Session;
use aura_model_reasoner::ThinkingEffort;
use aura_protocol::{
    AgentCapabilities, AgentIdentity, AgentPermissionsWire, ModelSelection, ReasoningEffort,
    RuntimeRequest, RuntimeRequestType, WorkspaceLocation,
};

#[test]
fn chat_effort_survives_worker_and_child_identity() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(directory.path().to_path_buf());
    session
        .apply_chat_runtime_request(RuntimeRequest {
            r#type: RuntimeRequestType::Chat {
                conversation_messages: vec![],
            },
            agent_identity: AgentIdentity::default(),
            model: ModelSelection {
                id: Some("test-selected-model".into()),
                reasoning_effort: Some(ReasoningEffort::XHigh),
                ..Default::default()
            },
            workspace: WorkspaceLocation::default(),
            project: None,
            agent_permissions: AgentPermissionsWire::default(),
            tool_permissions: None,
            agent_capabilities: AgentCapabilities::default(),
            auth_jwt: None,
            user_id: "audit-probe".into(),
        })
        .unwrap();
    let direct = session.agent_loop_config();
    let worker_or_child = session.as_runtime_identity().into_loop_config();
    assert_eq!(direct.user_thinking_effort, Some(ThinkingEffort::XHigh));
    assert_eq!(worker_or_child.model, direct.model);
    assert_eq!(worker_or_child.max_tokens, direct.max_tokens);
    assert_eq!(
        worker_or_child.user_thinking_effort,
        Some(ThinkingEffort::XHigh)
    );
}

#[tokio::test]
async fn exhausted_truncation_is_reported_to_ui_as_max_tokens() {
    use aura_agent::{
        types::{AgentToolExecutor, ToolCallInfo, ToolCallResult},
        AgentLoop, AgentLoopConfig,
    };
    use aura_model_reasoner::{Message, MockProvider, MockResponse, StopReason};
    struct NoTools;
    #[async_trait::async_trait]
    impl AgentToolExecutor for NoTools {
        async fn execute(&self, _: &[ToolCallInfo]) -> Vec<ToolCallResult> {
            vec![]
        }
    }
    let provider = MockProvider::new()
        .with_response(MockResponse::text("Partial answer").with_stop_reason(StopReason::MaxTokens))
        .with_response(MockResponse::text("More partial").with_stop_reason(StopReason::MaxTokens))
        .with_response(MockResponse::text("Still partial").with_stop_reason(StopReason::MaxTokens));
    let result = AgentLoop::new(AgentLoopConfig::for_agent("test-model"))
        .run(&provider, &NoTools, vec![Message::user("explain")], vec![])
        .await
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(directory.path().to_path_buf());
    let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
    super::helpers::apply_turn_result(&mut session, &result, "probe", &sender).await;
    let event = receiver.recv().await.unwrap();
    match event {
        aura_protocol::OutboundMessage::AssistantMessageEnd(end) => {
            assert_eq!(end.stop_reason, "max_tokens")
        }
        other => panic!("unexpected event: {other:?}"),
    }
}
