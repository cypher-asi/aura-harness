use super::*;
use aura_exec_tools::IntentClassifier;
use aura_model::ModelRequestKind;
use aura_model::ToolDefinition;
use serde_json::json;
use std::sync::Arc;

fn mk_tool(name: &str) -> ToolDefinition {
    ToolDefinition::new(name, name, json!({}))
}

fn mk_config_with_classifier() -> AgentLoopConfig {
    let classifier = IntentClassifier::from_rules(
        vec!["project".to_string()],
        vec![("billing".to_string(), vec!["credit".to_string()])],
    );
    AgentLoopConfig {
        intent_classifier: Some(Arc::new(classifier)),
        intent_classifier_manifest: vec![
            ("create_project".to_string(), "project".to_string()),
            ("list_credits".to_string(), "billing".to_string()),
        ],
        ..AgentLoopConfig::for_agent("claude-test-model")
    }
}

#[test]
fn build_request_filters_tier2_tools_when_not_triggered() {
    let config = mk_config_with_classifier();
    let state = LoopState::new(&config, vec![Message::user("hello there")]);
    let tools = vec![
        mk_tool("create_project"),
        mk_tool("list_credits"),
        mk_tool("read_file"),
    ];

    let req = state.build_request(&config, &tools, 1).unwrap();
    let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();

    assert!(names.contains(&"create_project"), "tier-1 tool kept");
    assert!(names.contains(&"read_file"), "unmapped tool passes through");
    assert!(
        !names.contains(&"list_credits"),
        "tier-2 billing tool hidden"
    );
}

#[test]
fn build_request_admits_tier2_when_keyword_matches() {
    let config = mk_config_with_classifier();
    let state = LoopState::new(
        &config,
        vec![Message::user("check my credit balance please")],
    );
    let tools = vec![mk_tool("create_project"), mk_tool("list_credits")];

    let req = state.build_request(&config, &tools, 1).unwrap();
    let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"list_credits"));
    assert!(names.contains(&"create_project"));
}

#[test]
fn build_request_skips_tool_result_messages_when_picking_intent() {
    let config = mk_config_with_classifier();
    let msgs = vec![
        Message::user("check my credit balance"),
        Message::assistant("calling tool"),
        Message::tool_results(vec![(
            "tu_1".into(),
            aura_model::ToolResultContent::Text("100".into()),
            false,
        )]),
    ];
    let state = LoopState::new(&config, msgs);
    let tools = vec![mk_tool("list_credits"), mk_tool("create_project")];

    let req = state.build_request(&config, &tools, 2).unwrap();
    let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&"list_credits"),
        "classifier should still see original user message after a tool-result turn"
    );
}

#[test]
fn build_request_passthrough_when_classifier_absent() {
    let config = AgentLoopConfig::for_agent("claude-test-model");
    let state = LoopState::new(&config, vec![Message::user("anything")]);
    let tools = vec![mk_tool("anything_tool")];
    let req = state.build_request(&config, &tools, 1).unwrap();
    assert_eq!(req.tools.len(), 1);
}

#[test]
fn build_request_keeps_tool_hints_scoped_after_first_iteration() {
    let config = AgentLoopConfig {
        tool_hints: Some(vec!["read_file".to_string(), "create_task".to_string()]),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let msgs = vec![
        Message::user("extract tasks"),
        Message::assistant("calling tool"),
        Message::tool_results(vec![(
            "tu_1".into(),
            aura_model::ToolResultContent::Text("large requirements".into()),
            false,
        )]),
    ];
    let state = LoopState::new(&config, msgs);
    let tools = vec![
        mk_tool("read_file"),
        mk_tool("create_task"),
        mk_tool("run_command"),
        mk_tool("generate_image"),
    ];

    let req = state.build_request(&config, &tools, 2).unwrap();
    let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();

    assert_eq!(names, vec!["read_file", "create_task"]);
    assert!(matches!(req.tool_choice, aura_model::ToolChoice::Auto));
}

#[test]
fn build_request_keeps_tool_hints_auto_on_first_iteration() {
    let config = AgentLoopConfig {
        tool_hints: Some(vec!["read_file".to_string(), "create_task".to_string()]),
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let state = LoopState::new(&config, vec![Message::user("extract tasks")]);
    let tools = vec![
        mk_tool("read_file"),
        mk_tool("create_task"),
        mk_tool("run_command"),
    ];

    let req = state.build_request(&config, &tools, 0).unwrap();
    let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();

    assert_eq!(names, vec!["read_file", "create_task"]);
    assert!(matches!(req.tool_choice, aura_model::ToolChoice::Auto));
}

/// Regression: a `Chat` request with `create_task` in scope must
/// keep `request_kind = Chat`, NOT silently get re-classified as
/// `ProjectToolTaskExtract`. The latter carries a 48 KiB total-text
/// budget in `aura-reasoner::content_profile`, so the old
/// reclassification turned every chat for an agent-with-task-tools
/// into a hard `EmergencyCapRequired` failure once history grew
/// past ~48 KiB. The fix narrows the override to dev-loop turns.
#[test]
fn build_request_keeps_chat_kind_when_task_tools_visible() {
    let config = AgentLoopConfig {
        request_kind: ModelRequestKind::Chat,
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let state = LoopState::new(&config, vec![Message::user("hi there")]);
    let tools = vec![mk_tool("create_task"), mk_tool("read_file")];

    let req = state.build_request(&config, &tools, 0).unwrap();
    assert_eq!(
            req.metadata.kind,
            Some(ModelRequestKind::Chat),
            "Chat must stay Chat even when task tools are visible (otherwise EmergencyCapRequired blocks chat at 48 KiB)"
        );
}

/// Companion: same invariant for spec tools — `create_spec` etc.
/// in scope must not flip a `Chat` turn into `ProjectToolSpecGen`.
#[test]
fn build_request_keeps_chat_kind_when_spec_tools_visible() {
    let config = AgentLoopConfig {
        request_kind: ModelRequestKind::Chat,
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let state = LoopState::new(&config, vec![Message::user("hi")]);
    let tools = vec![mk_tool("create_spec"), mk_tool("read_file")];

    let req = state.build_request(&config, &tools, 0).unwrap();
    assert_eq!(req.metadata.kind, Some(ModelRequestKind::Chat));
}

/// The dev-loop flow IS still subject to the project-tool override:
/// when the caller declares `DevLoopBootstrap` AND task tools are
/// available, the iteration after iteration `0` must report
/// `ProjectToolTaskExtract` (the existing extraction-phase guard).
/// Pins the narrowing didn't accidentally break the dev loop.
#[test]
fn build_request_promotes_devloop_to_project_tool_task_extract_when_task_tools_visible() {
    let config = AgentLoopConfig {
        request_kind: ModelRequestKind::DevLoopBootstrap,
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let state = LoopState::new(&config, vec![Message::user("extract tasks")]);
    let tools = vec![mk_tool("create_task")];

    let req = state.build_request(&config, &tools, 1).unwrap();
    assert_eq!(
        req.metadata.kind,
        Some(ModelRequestKind::ProjectToolTaskExtract)
    );
}

/// Mirror for the spec branch.
#[test]
fn build_request_promotes_devloop_to_project_tool_spec_gen_when_spec_tools_visible() {
    let config = AgentLoopConfig {
        request_kind: ModelRequestKind::DevLoopBootstrap,
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let state = LoopState::new(&config, vec![Message::user("extract specs")]);
    let tools = vec![mk_tool("create_spec")];

    let req = state.build_request(&config, &tools, 1).unwrap();
    assert_eq!(
        req.metadata.kind,
        Some(ModelRequestKind::ProjectToolSpecGen)
    );
}
