use super::*;
use crate::event_loop::ProcessTable;
use crate::session_helpers::default_agent_config_with_auth;
use aura_agent_kernel::{ChildAgentSpec, SpawnHook};
use aura_model::MockProvider;
use aura_store_db::{RocksStore, Store};
use aura_ui::events::ProcessKind;
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
        _process: aura_exec_tools::BackgroundProcess,
    ) -> Result<(), String> {
        Err("not in this test".into())
    }
}

fn agents(dir: &TempDir, reply: &str) -> TerminalAgents {
    agents_on(
        dir,
        Arc::new(MockProvider::simple_response(reply.to_string())),
    )
}

/// [`agents`] whose spawned agents' kernels call `provider`.
fn agents_on(
    dir: &TempDir,
    provider: Arc<dyn aura_model::ModelProvider + Send + Sync>,
) -> TerminalAgents {
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
    let scheduler = Arc::new(Scheduler::new(
        Arc::clone(&store),
        provider,
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

fn helper_spec() -> ChildAgentSpec {
    ChildAgentSpec {
        name: "helper".into(),
        role: "helper".into(),
        permissions: AgentPermissions::full_access(),
        tool_permissions: None,
        parent_tool_permissions: None,
        system_prompt_override: None,
        preassigned_agent_id: None,
        initial_prompt: Some("go".into()),
    }
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
        &RootTurn {
            config: &default_agent_config_with_auth("mock-model", None),
            mode: AgentMode::Agent,
            cache_salt: None,
            run_scratch: None,
        },
        notices,
    );
    table.add_root(root);

    let outcome = agents
        .hooks_for(root)
        .spawn_child(&root, None, helper_spec())
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
    // The reply stays in the thread the first prompt started, named after it.
    assert_eq!(notice.shown, "Message from helper: go");
    assert!(notice.prompt.contains("all done"), "{}", notice.prompt);
    assert!(
        agents.host().tree(root).len() > 1,
        "helper stays listed after replying: {outcome:?}"
    );
}

/// Answers like [`MockProvider`], keeping the cache salt of each request
/// it was sent.
#[derive(Default)]
struct SaltCapture {
    salts: std::sync::Mutex<Vec<Option<String>>>,
}

#[async_trait::async_trait]
impl aura_model::ModelProvider for SaltCapture {
    fn name(&self) -> &'static str {
        "salt-capture"
    }

    async fn complete(
        &self,
        request: aura_model::ModelRequest,
    ) -> Result<aura_model::ModelResponse, aura_model::ReasonerError> {
        self.salts.lock().unwrap().push(request.cache_salt.clone());
        MockProvider::simple_response("all done")
            .complete(request)
            .await
    }

    async fn health_check(&self) -> bool {
        true
    }
}

/// Spawn a helper under a root registered with `cache_salt`, wait for its
/// reply, and return the salts of the requests the helper sent.
async fn spawned_salts(cache_salt: Option<&str>) -> Vec<Option<String>> {
    let dir = TempDir::new().unwrap();
    let provider = Arc::new(SaltCapture::default());
    let agents = agents_on(&dir, provider.clone());
    let root = AgentId::generate();
    let (notices, mut notice_rx) = mpsc::unbounded_channel();
    let turn = RootTurn {
        config: &default_agent_config_with_auth("mock-model", None),
        mode: AgentMode::Agent,
        cache_salt,
        run_scratch: None,
    };
    agents.register(root, "Lead", dir.path().to_path_buf(), &turn, notices);
    let child = ChildAgentSpec {
        name: "helper".into(),
        role: "helper".into(),
        permissions: AgentPermissions::full_access(),
        tool_permissions: None,
        parent_tool_permissions: None,
        system_prompt_override: None,
        preassigned_agent_id: None,
        initial_prompt: Some("go".into()),
    };
    agents
        .hooks_for(root)
        .spawn_child(&root, None, child)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), notice_rx.recv())
        .await
        .expect("timed out waiting for the reply")
        .expect("notices closed");
    let salts = provider.salts.lock().unwrap().clone();
    salts
}

#[tokio::test]
async fn a_spawned_agent_shares_its_roots_cache_salt() {
    let salts = spawned_salts(Some("5a175a175a175a17")).await;
    assert!(!salts.is_empty());
    assert!(
        salts
            .iter()
            .all(|s| s.as_deref() == Some("5a175a175a175a17")),
        "{salts:?}"
    );

    let salts = spawned_salts(None).await;
    assert!(!salts.is_empty());
    assert!(
        salts.iter().all(Option::is_none),
        "an unsalted root's spawns: {salts:?}"
    );
}
