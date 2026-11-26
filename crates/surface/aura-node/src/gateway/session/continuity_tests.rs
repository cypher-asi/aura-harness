//! A cloud agent's conversation across node runs: each turn's reply is
//! recorded and checkpointed, and the next run, even on a restarted node,
//! continues it without starting a new session.

use super::chat_run::{spawn_chat_run, ChatRunHandle, ChatRunRegistry};
use super::{prepare_chat_session, WsContext};
use crate::protocol::{InboundMessage, OutboundMessage, UserMessage};
use aura_agent::checkpoint::Checkpoints;
use aura_core_protocol::{
    AgentCapabilities, AgentIdentity, AgentPermissionsWire, ModelSelection, RuntimeRequest,
    RuntimeRequestType, WorkspaceLocation,
};
use aura_core_types::{AgentId, RecordEntry, Transaction, TransactionType};
use aura_engine::scheduler::Scheduler;
use aura_exec_tools::{ToolCatalog, ToolConfig};
use aura_model::{
    ContentBlock, MockProvider, MockResponse, ModelProvider, ModelRequest, ModelResponse,
    ReasonerError,
};
use aura_store_db::{ReadStore, RocksStore, Store, WriteStore};
use dashmap::DashMap;
use parking_lot::Mutex;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Answers from a script and keeps every request it was sent.
struct Recording {
    script: MockProvider,
    requests: Mutex<Vec<ModelRequest>>,
}

impl Recording {
    fn new(responses: Vec<MockResponse>) -> Arc<Self> {
        Arc::new(Self {
            script: MockProvider::new().with_responses(responses),
            requests: Mutex::default(),
        })
    }

    /// The first request whose last message says `text`.
    fn request_for(&self, text: &str) -> ModelRequest {
        self.requests
            .lock()
            .iter()
            .find(|request| {
                request
                    .messages
                    .last()
                    .is_some_and(|m| m.text_content().contains(text))
            })
            .cloned()
            .unwrap_or_else(|| panic!("no request ends with {text:?}"))
    }
}

#[async_trait::async_trait]
impl ModelProvider for Recording {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        self.requests.lock().push(request.clone());
        self.script.complete(request).await
    }

    async fn health_check(&self) -> bool {
        true
    }
}

/// A node over `store` keeping checkpoints under `data_dir`: a fresh one
/// per run stands for a node process that restarted in between.
fn node(data_dir: &Path, store: &Arc<RocksStore>, provider: &Arc<Recording>) -> WsContext {
    let store: Arc<dyn Store> = store.clone();
    let provider: Arc<dyn ModelProvider + Send + Sync> = provider.clone();
    let workspace_base = data_dir.join("workspaces");
    let catalog = Arc::new(ToolCatalog::default());
    let scheduler = Arc::new(Scheduler::new(
        store.clone(),
        provider.clone(),
        Vec::new(),
        catalog.executor_builtin_tools(),
        workspace_base.clone(),
        None,
    ));
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
        chat_runs: Arc::new(DashMap::new()),
        run_id: None,
        agent_host,
        sessions: Some(Checkpoints::new(data_dir)),
    }
}

/// The run a hub starts for `agent`'s turn: no conversation of its own.
fn hub_request(agent: AgentId, workspace: &Path) -> RuntimeRequest {
    RuntimeRequest {
        r#type: RuntimeRequestType::Chat {
            conversation_messages: Vec::new(),
        },
        agent_identity: AgentIdentity {
            partition_id: Some(agent.to_hex()),
            ..AgentIdentity::default()
        },
        model: ModelSelection {
            id: Some("mock-model".to_string()),
            ..ModelSelection::default()
        },
        workspace: WorkspaceLocation {
            workspace: Some(workspace.display().to_string()),
            ..WorkspaceLocation::default()
        },
        project: None,
        agent_permissions: AgentPermissionsWire::default(),
        tool_permissions: None,
        agent_capabilities: AgentCapabilities::default(),
        auth_jwt: None,
        user_id: "hub".to_string(),
        mode: None,
    }
}

/// Run one turn saying `prompt` in a run of its own, as the hub does,
/// and stop the run once the turn has ended.
async fn hub_turn(ctx: WsContext, agent: AgentId, run_id: &str, prompt: &str) {
    let workspace = ctx.workspace_base.join("agent");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(workspace.join("notes.txt"), "hello from the notes").expect("notes");
    let session = prepare_chat_session(hub_request(agent, &workspace), &ctx)
        .await
        .expect("prepare");
    let registry: ChatRunRegistry = Arc::new(DashMap::new());
    let handle = spawn_chat_run(session, ctx, run_id.to_string(), registry.clone());
    handle
        .commands
        .send(InboundMessage::UserMessage(UserMessage {
            content: prompt.into(),
            tool_hints: None,
            attachments: None,
        }))
        .await
        .expect("send");
    wait_for_turn_end(&handle).await;
    handle.shutdown.cancel();
    for _ in 0..250 {
        if !registry.contains_key(run_id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("run {run_id} did not stop");
}

async fn wait_for_turn_end(handle: &ChatRunHandle) {
    for _ in 0..750 {
        let history = handle.events.subscribe().history;
        if history
            .iter()
            .any(|m| matches!(m, OutboundMessage::AssistantMessageEnd(_)))
        {
            return;
        }
        if let Some(error) = history.iter().find_map(|m| match m {
            OutboundMessage::Error(e) => Some(format!("{e:?}")),
            _ => None,
        }) {
            panic!("the turn failed: {error}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for the turn to end");
}

fn record(store: &RocksStore, agent: AgentId) -> Vec<RecordEntry> {
    store.scan_record(agent, 1, 1000).expect("scan")
}

fn count(entries: &[RecordEntry], kind: TransactionType) -> usize {
    entries.iter().filter(|e| e.tx.tx_type == kind).count()
}

fn capability_rows(entries: &[RecordEntry]) -> usize {
    entries
        .iter()
        .filter(|e| {
            e.tx.tx_type == TransactionType::System
                && String::from_utf8_lossy(&e.tx.payload).contains("\"capability_install\"")
        })
        .count()
}

fn has_tool_call(request: &ModelRequest, id: &str) -> (bool, bool) {
    let blocks = || request.messages.iter().flat_map(|m| m.content.iter());
    (
        blocks().any(|b| matches!(b, ContentBlock::ToolUse { id: used, .. } if used == id)),
        blocks().any(
            |b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id),
        ),
    )
}

fn says(request: &ModelRequest, text: &str) -> bool {
    request
        .messages
        .iter()
        .any(|m| m.text_content().contains(text))
}

#[tokio::test]
async fn the_next_run_continues_the_conversation_after_a_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(RocksStore::open(dir.path().join("db"), false).expect("store"));
    let provider = Recording::new(vec![
        MockResponse::tool_use(
            "t1",
            "read_file",
            serde_json::json!({ "path": "notes.txt" }),
        ),
        MockResponse::text("The notes say hello."),
        MockResponse::text("Still hello."),
    ]);
    let agent = AgentId::new([7; 32]);

    hub_turn(
        node(dir.path(), &store, &provider),
        agent,
        "run-1",
        "What do the notes say?",
    )
    .await;

    let first = record(&store, agent);
    assert_eq!(count(&first, TransactionType::SessionStart), 1);
    let replies: Vec<String> = first
        .iter()
        .filter(|e| e.tx.tx_type == TransactionType::AgentMsg)
        .map(|e| String::from_utf8_lossy(&e.tx.payload).into_owned())
        .collect();
    assert_eq!(replies, ["The notes say hello."], "the reply is recorded");
    let saved = Checkpoints::new(dir.path())
        .load_current(agent)
        .expect("a checkpoint");
    assert_eq!(saved.record_seq, store.get_head_seq(agent).expect("head"));
    assert!(saved.messages.iter().any(|m| m
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse { id, .. } if id == "t1"))));

    hub_turn(
        node(dir.path(), &store, &provider),
        agent,
        "run-2",
        "And now?",
    )
    .await;

    let second = record(&store, agent);
    assert_eq!(
        count(&second, TransactionType::SessionStart),
        1,
        "a continued conversation is not a new session"
    );
    assert_eq!(
        capability_rows(&second),
        capability_rows(&first),
        "unchanged capabilities are not recorded again"
    );
    let request = provider.request_for("And now?");
    assert!(says(&request, "What do the notes say?"), "{request:?}");
    assert!(says(&request, "The notes say hello."), "{request:?}");
    assert_eq!(
        has_tool_call(&request, "t1"),
        (true, true),
        "the earlier tool call comes along with its result"
    );
    let resaved = Checkpoints::new(dir.path())
        .load_current(agent)
        .expect("a checkpoint");
    assert_eq!(resaved.started_ms, saved.started_ms, "the same session");
    assert_eq!(resaved.usage.calls, saved.usage.calls + 1);
}

#[tokio::test]
async fn without_a_checkpoint_the_record_carries_the_conversation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(RocksStore::open(dir.path().join("db"), false).expect("store"));
    let agent = AgentId::new([8; 32]);
    // What a hand-off imports from the app: the agent's record so far.
    let imported = [
        Transaction::session_start(agent),
        Transaction::user_prompt(agent, "Remember the number 42."),
        Transaction::new_chained(
            agent,
            TransactionType::AgentMsg,
            b"Noted: 42.".to_vec(),
            None,
        ),
    ];
    for (seq, tx) in (1..).zip(imported) {
        let entry = RecordEntry::builder(seq, tx).build();
        store
            .append_entry_direct(agent, seq, &entry)
            .expect("append");
    }
    let provider = Recording::new(vec![MockResponse::text("It was 42.")]);

    hub_turn(
        node(dir.path(), &store, &provider),
        agent,
        "run-1",
        "Which number?",
    )
    .await;

    let request = provider.request_for("Which number?");
    assert!(says(&request, "Remember the number 42."), "{request:?}");
    assert!(says(&request, "Noted: 42."), "{request:?}");
    assert_eq!(
        count(&record(&store, agent), TransactionType::SessionStart),
        1,
        "the imported session goes on"
    );
}
