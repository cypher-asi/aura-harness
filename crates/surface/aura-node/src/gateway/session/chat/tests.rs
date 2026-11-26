use super::turn::cloud_git_note;
use super::*;
use aura_core_types::{
    AgentPermissions, Capability, InstalledIntegrationDefinition, InstalledToolDefinition,
    InstalledToolIntegrationRequirement, ToolAuth,
};
use aura_engine::scheduler::Scheduler;
use aura_exec_tools::{ToolCatalog, ToolConfig};
use aura_model::MockProvider;
use aura_store_db::RocksStore;
use std::collections::HashMap;
use std::sync::Arc;

fn test_context() -> WsContext {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let db_dir = tempfile::tempdir().expect("temp db");
    let store = Arc::new(RocksStore::open(db_dir.path(), false).expect("open rocks store"));
    let provider = Arc::new(MockProvider::simple_response("ok"));
    let workspace_base = workspace.path().to_path_buf();
    let catalog = Arc::new(ToolCatalog::default());
    let scheduler = Arc::new(Scheduler::new(
        store.clone(),
        provider.clone(),
        Vec::new(),
        catalog.executor_builtin_tools(),
        workspace_base.clone(),
        None,
    ));
    std::mem::forget(workspace);
    std::mem::forget(db_dir);

    let agent_host = aura_engine::AgentHost::new(
        store.clone(),
        scheduler.clone(),
        aura_engine::AgentHostConfig::default(),
    );

    WsContext {
        workspace_base,
        provider,
        store,
        scheduler,
        tool_config: ToolConfig::default(),
        auth_token: None,
        catalog,
        domain_api: None,
        automaton_controller: None,
        project_base: None,
        memory_manager: None,
        skill_manager: None,
        router_url: None,
        aura_os_server_url: None,
        chat_runs: Arc::new(dashmap::DashMap::new()),
        run_id: None,
        agent_host,
    }
}

fn gated_tool() -> InstalledToolDefinition {
    InstalledToolDefinition {
        name: "brave_search_web".to_string(),
        description: "Search the web with Brave".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        }),
        endpoint: "https://example.com/tool".to_string(),
        auth: ToolAuth::None,
        timeout_ms: None,
        namespace: None,
        required_integration: Some(InstalledToolIntegrationRequirement {
            integration_id: None,
            provider: Some("brave_search".to_string()),
            kind: Some("workspace_integration".to_string()),
        }),
        runtime_execution: None,
        metadata: HashMap::new(),
    }
}

fn brave_integration() -> InstalledIntegrationDefinition {
    InstalledIntegrationDefinition {
        integration_id: "integration-brave-1".to_string(),
        name: "Brave Search".to_string(),
        provider: "brave_search".to_string(),
        kind: "workspace_integration".to_string(),
        metadata: HashMap::new(),
    }
}

const CROSS_AGENT_TOOLS: [&str; 7] = [
    "send_to_agent",
    "spawn_agent",
    "agent_lifecycle",
    "get_agent_state",
    "list_agents",
    "delegate_task",
    "task",
];

fn cross_agent_tool_names(session: &Session) -> Vec<String> {
    session
        .tool_definitions
        .iter()
        .filter(|tool| CROSS_AGENT_TOOLS.contains(&tool.name.as_str()))
        .map(|tool| tool.name.clone())
        .collect()
}

fn assert_cross_agent_tools(session: &Session, expected: &[&str]) {
    let mut actual = cross_agent_tool_names(session);
    actual.sort();
    let mut expected = expected
        .iter()
        .map(|tool| (*tool).to_string())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(actual, expected);
}

#[test]
fn populate_tool_definitions_hides_integration_backed_tool_without_install() {
    let ctx = test_context();
    let mut session = Session::new(ctx.workspace_base.clone());
    session.installed_tools.push(gated_tool());

    populate_tool_definitions(&mut session, &ctx);

    assert!(!session
        .tool_definitions
        .iter()
        .any(|tool| tool.name == "brave_search_web"));
}

#[test]
fn populate_tool_definitions_keeps_integration_backed_tool_with_install() {
    let ctx = test_context();
    let mut session = Session::new(ctx.workspace_base.clone());
    session.installed_tools.push(gated_tool());
    session.installed_integrations.push(brave_integration());

    populate_tool_definitions(&mut session, &ctx);

    assert!(session
        .tool_definitions
        .iter()
        .any(|tool| tool.name == "brave_search_web"));
}

#[test]
fn populate_tool_definitions_includes_ceo_cross_agent_tools() {
    let ctx = test_context();
    let mut session = Session::new(ctx.workspace_base.clone());
    session.agent_permissions = AgentPermissions::ceo_preset();

    populate_tool_definitions(&mut session, &ctx);

    assert_cross_agent_tools(&session, &CROSS_AGENT_TOOLS);
}

#[test]
fn populate_tool_definitions_filters_cross_agent_tools_for_control_agent() {
    let ctx = test_context();
    let mut session = Session::new(ctx.workspace_base.clone());
    session.agent_permissions = AgentPermissions {
        scope: Default::default(),
        capabilities: vec![Capability::ControlAgent],
    };

    populate_tool_definitions(&mut session, &ctx);

    assert_cross_agent_tools(
        &session,
        &["send_to_agent", "agent_lifecycle", "delegate_task"],
    );
}

#[test]
fn populate_tool_definitions_filters_cross_agent_tools_for_spawn_agent() {
    let ctx = test_context();
    let mut session = Session::new(ctx.workspace_base.clone());
    session.agent_permissions = AgentPermissions {
        scope: Default::default(),
        capabilities: vec![Capability::SpawnAgent],
    };

    populate_tool_definitions(&mut session, &ctx);

    assert_cross_agent_tools(&session, &["spawn_agent", "task"]);
}

#[test]
fn populate_tool_definitions_filters_cross_agent_tools_for_read_agent() {
    let ctx = test_context();
    let mut session = Session::new(ctx.workspace_base.clone());
    session.agent_permissions = AgentPermissions {
        scope: Default::default(),
        capabilities: vec![Capability::ReadAgent],
    };

    populate_tool_definitions(&mut session, &ctx);

    assert_cross_agent_tools(&session, &["get_agent_state"]);
}

#[test]
fn populate_tool_definitions_filters_cross_agent_tools_for_list_agents() {
    let ctx = test_context();
    let mut session = Session::new(ctx.workspace_base.clone());
    session.agent_permissions = AgentPermissions {
        scope: Default::default(),
        capabilities: vec![Capability::ListAgents],
    };

    populate_tool_definitions(&mut session, &ctx);

    assert_cross_agent_tools(&session, &["list_agents"]);
}

#[test]
fn the_cloud_git_note_names_the_branch_and_the_tools() {
    let note = cloud_git_note("aura/reviewer-re36");
    assert!(note.contains("`aura/reviewer-re36`"), "{note}");
    assert!(
        note.contains("git_push") && note.contains("gh pr create"),
        "{note}"
    );
}
