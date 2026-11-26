//! Shared fixtures for the session helper tests.

use crate::gateway::session::WsContext;
use aura_core_protocol::{
    AgentCapabilities, AgentIdentity, AgentPermissionsWire, ModelSelection, RuntimeRequest,
    RuntimeRequestType, SessionModelOverrides, WorkspaceLocation,
};
use aura_engine::scheduler::Scheduler;
use aura_exec_tools::{ToolCatalog, ToolConfig};
use aura_model::MockProvider;
use aura_store_db::RocksStore;
use std::sync::Arc;

pub(super) fn test_context() -> WsContext {
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

pub(super) fn chat_request(
    workspace: Option<String>,
    provider_overrides: Option<SessionModelOverrides>,
) -> RuntimeRequest {
    RuntimeRequest {
        r#type: RuntimeRequestType::Chat {
            conversation_messages: Vec::new(),
        },
        agent_identity: AgentIdentity::default(),
        model: ModelSelection {
            provider_overrides,
            ..ModelSelection::default()
        },
        workspace: WorkspaceLocation {
            workspace,
            project_path: None,
            git_repo_url: None,
            git_branch: None,
        },
        project: None,
        agent_permissions: AgentPermissionsWire::default(),
        tool_permissions: None,
        agent_capabilities: AgentCapabilities::default(),
        auth_jwt: None,
        user_id: "user-test".to_string(),
        mode: None,
    }
}
