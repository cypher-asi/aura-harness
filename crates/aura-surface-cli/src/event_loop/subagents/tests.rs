use super::*;
use crate::session_helpers::{build_executor_router_with_config, default_agent_config_with_auth};
use aura_agent_kernel::{Kernel, KernelConfig};
use aura_context_skills::SkillLoader;
use aura_core_types::{AgentPermissions, UserToolDefaults};
use aura_model_reasoner::{MockProvider, ModelProvider};
use aura_store_db::RocksStore;
use aura_tools::ToolConfig;
use tempfile::TempDir;

struct Fixture {
    hub: Arc<AgentHub>,
    dispatcher: Arc<dyn SubagentDispatchHook>,
    commands: mpsc::Receiver<UiCommand>,
    _data: TempDir,
    _db: TempDir,
    _workspace: TempDir,
}

/// A hub over seeded permanent agents whose kernels all answer `reply`.
fn fixture(reply: &str) -> Fixture {
    let data = TempDir::new().unwrap();
    let db = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let store = Arc::new(RocksStore::open(db.path(), false).unwrap());
    let (_, tools) = build_executor_router_with_config(&ToolConfig::for_autonomous_dev_loop());
    let config = default_agent_config_with_auth("mock-model", None);
    let (tx, commands) = mpsc::channel(64);
    let parts = AgentHubParts {
        registry: AgentRegistry::load_or_init(data.path()).unwrap(),
        definitions: AgentDefinitions::load_or_seed(data.path()).unwrap(),
        skills: SkillManager::new(SkillLoader::with_defaults(
            Some(workspace.path().to_path_buf()),
            None,
        )),
        base: TurnBase {
            limits: ModelLimits::of(&config),
            config,
            tools: tools
                .into_iter()
                .filter(|t| t.name != TASK_TOOL_NAME)
                .collect(),
        },
        commands: tx,
    };
    let reply = reply.to_string();
    let workspace_root = workspace.path().to_path_buf();
    let mut captured = None;
    let hub = AgentHub::new(parts, |dispatcher| {
        captured = Some(dispatcher);
        let kernel_store = Arc::clone(&store);
        KernelFactory::new(
            store,
            Box::new(move |agent_id, _parent_chain| {
                let provider: Arc<dyn ModelProvider + Send + Sync> =
                    Arc::new(MockProvider::simple_response(reply.clone()));
                Ok(Kernel::new(
                    Arc::clone(&kernel_store) as Arc<dyn aura_store_db::Store>,
                    provider,
                    build_executor_router_with_config(&ToolConfig::for_autonomous_dev_loop()).0,
                    KernelConfig {
                        workspace_base: workspace_root.clone(),
                        use_workspace_base_as_root: true,
                        ..KernelConfig::default()
                    },
                    agent_id,
                )?)
            }),
        )
    });
    Fixture {
        hub,
        dispatcher: captured.unwrap(),
        commands,
        _data: data,
        _db: db,
        _workspace: workspace,
    }
}

fn request(parent: AgentId, subagent_type: &str, prompt: &str) -> SubagentDispatchRequest {
    serde_json::from_value(serde_json::json!({
        "parent_agent_id": parent,
        "subagent_type": subagent_type,
        "prompt": prompt,
        "parent_permissions": AgentPermissions::full_access(),
        "user_tool_defaults": UserToolDefaults::default(),
    }))
    .unwrap()
}

#[test]
fn task_tool_lists_permanent_agents() {
    let fixture = fixture("unused");
    let tool = task_tool_definition(&lock(&fixture.hub.definitions)).unwrap();

    assert_eq!(tool.name, TASK_TOOL_NAME);
    assert_eq!(
        tool.input_schema["properties"]["subagent_type"]["enum"],
        serde_json::json!(["explorer", "reviewer"])
    );
    assert!(tool.description.contains("- reviewer: "));
}

#[test]
fn no_task_tool_without_permanent_agents() {
    let data = TempDir::new().unwrap();
    std::fs::create_dir_all(data.path().join("agent_defs")).unwrap();
    let definitions = AgentDefinitions::load_or_seed(data.path()).unwrap();

    assert!(task_tool_definition(&definitions).is_none());
}

#[test]
fn turn_applies_definition_rules_and_tools() {
    let fixture = fixture("unused");

    let (_, plain_tools) = fixture.hub.turn(None);
    assert!(plain_tools.iter().any(|t| t.name == TASK_TOOL_NAME));

    let (config, tools) = fixture.hub.turn(Some("explorer"));
    assert!(config
        .system_prompt
        .contains("<agent_definition name=\"explorer\">"));
    assert!(tools.iter().all(|t| t.name != TASK_TOOL_NAME));
    assert!(tools.iter().any(|t| t.name == "read_file"));
    assert!(tools.iter().all(|t| t.name != "write_file"));
}

#[tokio::test]
async fn unknown_permanent_agent_is_rejected() {
    let fixture = fixture("unused");
    let parent = lock(&fixture.hub.registry).active().agent_id();

    let result = fixture
        .dispatcher
        .dispatch(request(parent, "nobody", "hi"))
        .await
        .unwrap();

    assert!(
        matches!(&result.exit, SubagentExit::Rejected { reason } if reason.contains("explorer, reviewer"))
    );
    assert_eq!(lock(&fixture.hub.registry).agents().len(), 1);
}

#[tokio::test]
async fn spawn_runs_an_instance_under_its_parent() {
    let mut fixture = fixture("Looks good.");
    let parent = lock(&fixture.hub.registry).active().clone();

    let result = fixture
        .dispatcher
        .dispatch(request(parent.agent_id(), "reviewer", "Review the diff"))
        .await
        .unwrap();

    assert!(matches!(result.exit, SubagentExit::Completed));
    assert_eq!(result.final_message, "Looks good.");
    let child_id = result.child_agent_id.unwrap();
    {
        let registry = lock(&fixture.hub.registry);
        let child = registry.find(child_id).unwrap();
        assert_eq!(child.name, "reviewer #1");
        assert_eq!(child.definition.as_deref(), Some("reviewer"));
        assert_eq!(child.parent.as_deref(), Some(parent.zns_id.as_str()));
        assert_eq!(registry.active().agent_id(), parent.agent_id());
    }

    let finished = fixture.hub.take_finished();
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].agent_id(), child_id);
    assert_eq!(finished[0].definition.as_deref(), Some("reviewer"));
    assert_eq!(finished[0].messages.len(), 2);
    assert!(fixture.hub.take_finished().is_empty());

    let summaries = fixture.hub.agent_summaries();
    let child_row = summaries.iter().find(|s| s.name == "reviewer #1").unwrap();
    assert_eq!(child_row.depth, 1);
    assert!(!child_row.running);

    let mut saw_running = false;
    while let Ok(command) = fixture.commands.try_recv() {
        if let UiCommand::SetAgents(rows) = command {
            saw_running |= rows.iter().any(|r| r.name == "reviewer #1" && r.running);
        }
    }
    assert!(saw_running);
}
