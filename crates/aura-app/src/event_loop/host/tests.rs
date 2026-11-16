use super::*;
use crate::event_loop::ProcessTable;
use crate::session_helpers::default_agent_config_with_auth;
use aura_agent_kernel::{ChildAgentSpec, SpawnHook};
use aura_model_reasoner::MockProvider;
use aura_store_db::{RocksStore, Store};
use aura_terminal::events::ProcessKind;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

struct NoBackground;

impl BackgroundProcessHook for NoBackground {
    fn log_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }

    fn adopt(
        &self,
        _agent: AgentId,
        _process: aura_tools::BackgroundProcess,
    ) -> Result<(), String> {
        Err("not in this test".into())
    }
}

fn agents(dir: &TempDir, reply: &str) -> TerminalAgents {
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
    let scheduler = Arc::new(Scheduler::new(
        Arc::clone(&store),
        Arc::new(MockProvider::simple_response(reply.to_string())),
        Vec::new(),
        Vec::new(),
        dir.path().to_path_buf(),
        None,
    ));
    let host = AgentHost::new(store, Arc::clone(&scheduler), Default::default());
    TerminalAgents::new(
        host,
        scheduler,
        Arc::new(TerminalChildKernels::new(
            ToolConfig::default(),
            Arc::new(NoBackground),
        )),
    )
}

#[test]
fn agent_tools_leave_out_delegate_task_and_task() {
    let names: Vec<String> = agent_tool_definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect();

    for name in [
        "spawn_agent",
        "send_to_agent",
        "agent_lifecycle",
        "list_agents",
    ] {
        assert!(names.iter().any(|n| n == name), "{name} missing: {names:?}");
    }
    assert!(!names.iter().any(|n| n == "delegate_task"), "{names:?}");
    assert!(!names.iter().any(|n| n == TASK_TOOL_NAME), "{names:?}");
}

#[tokio::test]
async fn spawned_agent_replies_with_a_notice_and_is_listed() {
    let dir = TempDir::new().unwrap();
    let agents = agents(&dir, "all done");
    let table = ProcessTable::without_os_scan(Some(agents.host().clone()), None);
    let root = AgentId::generate();
    let (notices, mut notice_rx) = mpsc::unbounded_channel();
    agents.register(
        root,
        "Lead",
        dir.path().to_path_buf(),
        &default_agent_config_with_auth("mock-model", None),
        AgentMode::Agent,
        notices,
    );
    table.add_root(root);

    let outcome = agents
        .hooks_for(root)
        .spawn_child(
            &root,
            None,
            ChildAgentSpec {
                name: "helper".into(),
                role: "helper".into(),
                permissions: AgentPermissions::full_access(),
                tool_permissions: None,
                parent_tool_permissions: None,
                system_prompt_override: None,
                preassigned_agent_id: None,
                initial_prompt: Some("go".into()),
            },
        )
        .await
        .unwrap();

    let rows = table.snapshot();
    let child = rows
        .iter()
        .find(|row| row.kind == ProcessKind::Agent)
        .unwrap_or_else(|| panic!("the spawned agent should be listed: {rows:?}"));
    assert!(child.label.contains("helper"), "{child:?}");
    assert_eq!(child.owner_agent_id, root.to_hex());

    let notice = tokio::time::timeout(Duration::from_secs(20), notice_rx.recv())
        .await
        .expect("timed out waiting for the reply")
        .expect("notices closed");
    assert_eq!(notice.to, root);
    assert_eq!(notice.shown, "Message from helper");
    assert!(notice.prompt.contains("all done"), "{}", notice.prompt);
    assert!(
        agents.host().tree(root).len() > 1,
        "helper stays listed after replying: {outcome:?}"
    );
}
