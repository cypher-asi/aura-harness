//! The `task` tool: validation, and what it hands the dispatcher.

use super::*;
use crate::sandbox::Sandbox;
use crate::ToolConfig;
use aura_core_types::{AgentId, AgentPermissions, AgentScope};

fn ctx(caller: AgentPermissions) -> ToolContext {
    let dir = std::env::temp_dir();
    let mut ctx = ToolContext::new(Sandbox::new(&dir).unwrap(), ToolConfig::default());
    ctx.caller_permissions = Some(caller);
    ctx.caller_agent_id = Some(AgentId::generate());
    ctx.originating_user_id = Some("user-root".into());
    ctx
}

fn minimal_input() -> TaskInput {
    TaskInput {
        subagent_type: "explore".into(),
        prompt: "inspect".into(),
        model_override: None,
        system_prompt_addendum: None,
        mode: None,
        permissions: None,
        model: None,
        budget: None,
        isolation: None,
        tool_subset: None,
        tool_call_id: None,
        spawn_mode: None,
        check: None,
        todo_id: None,
        description: None,
    }
}

#[test]
fn task_requires_spawn_capability() {
    let caller = AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::ReadAgent],
    };
    let input = minimal_input();
    let err = TaskTool::build_request(&ctx(caller), &input).unwrap_err();
    assert!(err.to_string().contains("SpawnAgent"), "got: {err}");
}

#[test]
fn task_rejects_depth_limit() {
    let mut ctx = ctx(AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::SpawnAgent],
    });
    ctx.parent_chain = (0..MAX_SUBAGENT_DEPTH)
        .map(|_| AgentId::generate())
        .collect();
    let input = minimal_input();
    let err = TaskTool::build_request(&ctx, &input).unwrap_err();
    assert!(
        err.to_string().contains("maximum subagent depth"),
        "got: {err}"
    );
}

#[test]
fn task_uses_context_tool_use_id_as_dispatch_key() {
    let mut ctx = ctx(AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::SpawnAgent],
    });
    ctx.current_tool_use_id = Some("toolu_real_123".into());
    let mut input = minimal_input();
    input.tool_call_id = Some("model_supplied_dedupe".into());
    let request = TaskTool::build_request(&ctx, &input).unwrap();
    assert_eq!(request.tool_call_id.as_deref(), Some("toolu_real_123"));
}

#[test]
fn task_passes_its_check_to_the_dispatcher() {
    let ctx = ctx(AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::SpawnAgent],
    });
    let mut input = minimal_input();
    input.check = Some("  cargo test -p parser ".into());
    let request = TaskTool::build_request(&ctx, &input).unwrap();
    assert_eq!(request.check.as_deref(), Some("cargo test -p parser"));

    input.check = Some("   ".into());
    let request = TaskTool::build_request(&ctx, &input).unwrap();
    assert_eq!(request.check, None, "a blank check is no check");
    assert!(TaskTool::definition().input_schema["properties"]["check"].is_object());
}

#[test]
fn task_passes_its_description_to_the_dispatcher() {
    let ctx = ctx(AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::SpawnAgent],
    });
    let mut input: TaskInput = serde_json::from_value(serde_json::json!({
        "subagent_type": "explore",
        "prompt": "In the Rust workspace, find the title code.",
        "description": " Subagent title source ",
    }))
    .unwrap();
    let request = TaskTool::build_request(&ctx, &input).unwrap();
    assert_eq!(
        request.description.as_deref(),
        Some("Subagent title source")
    );

    input.description = Some("  ".into());
    let request = TaskTool::build_request(&ctx, &input).unwrap();
    assert_eq!(request.description, None, "a blank description is none");
    let schema = TaskTool::definition().input_schema;
    assert_eq!(
        schema["properties"]["description"]["description"],
        TASK_DESCRIPTION_HELP
    );
}

#[test]
fn task_falls_back_to_input_tool_call_id() {
    let mut ctx = ctx(AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::SpawnAgent],
    });
    ctx.current_tool_use_id = None;
    let mut input = minimal_input();
    input.tool_call_id = Some("model_supplied_dedupe".into());
    let request = TaskTool::build_request(&ctx, &input).unwrap();
    assert_eq!(
        request.tool_call_id.as_deref(),
        Some("model_supplied_dedupe")
    );
}

#[tokio::test]
async fn task_fails_closed_without_dispatch_hook() {
    let ctx = ctx(AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::SpawnAgent],
    });
    let result = TaskTool
        .execute(
            &ctx,
            serde_json::json!({
                "subagent_type": "explore",
                "prompt": "inspect"
            }),
        )
        .await
        .unwrap();
    assert!(!result.ok);
    let outcome: SubagentResult = serde_json::from_slice(&result.stderr).unwrap();
    assert!(matches!(
        outcome.exit,
        aura_core_types::SubagentExit::Rejected { .. }
    ));
}
