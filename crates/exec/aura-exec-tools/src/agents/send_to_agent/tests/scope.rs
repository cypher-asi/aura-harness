use super::*;

#[test]
fn send_to_agent_requires_control_capability() {
    let caller = AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::ReadAgent],
    };
    let input = SendToAgentInput {
        agent_id: "aa".into(),
        content: "hello".into(),
        attachments: None,
        subject: None,
    };
    let err = SendToAgentTool::evaluate(&ctx(caller), &input).unwrap_err();
    assert!(err.to_string().contains("permissions:"), "got: {err}");
    assert!(err.to_string().contains("ControlAgent"), "got: {err}");
}

#[test]
fn send_to_agent_denies_out_of_scope_target() {
    let caller = AgentPermissions {
        scope: AgentScope {
            agent_ids: vec!["allowed".into()],
            ..AgentScope::default()
        },
        capabilities: vec![Capability::ControlAgent],
    };
    let input = SendToAgentInput {
        agent_id: "not-allowed".into(),
        content: "hello".into(),
        attachments: None,
        subject: None,
    };
    let err = SendToAgentTool::evaluate(&ctx(caller), &input).unwrap_err();
    assert!(err.to_string().contains("permissions:"), "got: {err}");
    assert!(err.to_string().contains("AgentScope"), "got: {err}");
}

#[test]
fn send_to_agent_allows_in_scope_target() {
    let caller = AgentPermissions {
        scope: AgentScope {
            agent_ids: vec!["target-id".into()],
            ..AgentScope::default()
        },
        capabilities: vec![Capability::ControlAgent],
    };
    let input = SendToAgentInput {
        agent_id: "target-id".into(),
        content: "hello".into(),
        attachments: None,
        subject: None,
    };
    let outcome = SendToAgentTool::evaluate(&ctx(caller), &input).unwrap();
    assert_eq!(outcome.target_agent_id, "target-id");
    assert_eq!(outcome.originating_user_id.as_deref(), Some("user-root"));
    assert!(
        !outcome.delivered,
        "no hook wired — runtime side-effect skipped"
    );
}

#[test]
fn send_to_agent_universe_scope_allows_any_target() {
    let caller = AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::ControlAgent],
    };
    let input = SendToAgentInput {
        agent_id: "anything".into(),
        content: "hi".into(),
        attachments: None,
        subject: None,
    };
    assert!(SendToAgentTool::evaluate(&ctx(caller), &input).is_ok());
}
