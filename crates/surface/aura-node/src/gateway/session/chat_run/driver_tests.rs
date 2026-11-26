use super::*;
use crate::gateway::session::{prepare_chat_session, WsContext};
use crate::protocol::{InboundMessage, OutboundMessage, UserMessage};
use aura_core_protocol::{
    AgentCapabilities, AgentIdentity, AgentPermissionsWire, ModelSelection, RuntimeRequest,
    RuntimeRequestType, WorkspaceLocation,
};
use aura_engine::scheduler::Scheduler;
use aura_exec_tools::{ToolCatalog, ToolConfig};
use aura_model::{MockProvider, ModelProvider};
use aura_store_db::RocksStore;

fn driver_ctx() -> WsContext {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let db_dir = tempfile::tempdir().expect("temp db");
    let store = Arc::new(RocksStore::open(db_dir.path(), false).expect("open rocks store"));
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("ok"));
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
        chat_runs: Arc::new(DashMap::new()),
        run_id: None,
        agent_host,
    }
}

fn chat_request(workspace: String) -> RuntimeRequest {
    RuntimeRequest {
        r#type: RuntimeRequestType::Chat {
            conversation_messages: Vec::new(),
        },
        agent_identity: AgentIdentity::default(),
        model: ModelSelection::default(),
        workspace: WorkspaceLocation {
            workspace: Some(workspace),
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

/// Poll the channel's replay history until `pred` matches one of its
/// messages or the deadline elapses.
async fn wait_for_history(
    events: &Arc<ChatEventChannel>,
    pred: impl Fn(&OutboundMessage) -> bool,
    label: &str,
) {
    for _ in 0..600 {
        if events.subscribe().history.iter().any(&pred) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {label}");
}

/// End-to-end (no model network): a run spawned via `spawn_chat_run`
/// executes a turn, emits frames into its replay channel, survives
/// across turns, and a fresh attach (reattach) replays the full
/// history including the completed turn's `AssistantMessageEnd`.
#[tokio::test]
async fn driver_runs_turn_and_completed_history_is_reattachable() {
    let ctx = driver_ctx();
    let ws_path = ctx.workspace_base.join("drv");
    std::fs::create_dir_all(&ws_path).expect("workspace dir");
    let session = prepare_chat_session(chat_request(ws_path.display().to_string()), &ctx)
        .await
        .expect("prepare chat session");

    let registry: ChatRunRegistry = Arc::new(DashMap::new());
    let handle = spawn_chat_run(session, ctx, "run-1".to_string(), registry.clone());

    // Simulate one live WS attach so the idle-reaper never fires
    // mid-test (and to exercise attach-count keeping the run alive).
    let _attach = AttachGuard::new(handle.attach_count.clone());

    // First attach observes SessionReady.
    wait_for_history(
        &handle.events,
        |m| matches!(m, OutboundMessage::SessionReady(_)),
        "SessionReady",
    )
    .await;

    // Drive a turn.
    handle
        .commands
        .send(InboundMessage::UserMessage(UserMessage {
            content: "hi".into(),
            tool_hints: None,
            attachments: None,
        }))
        .await
        .expect("send user message");

    wait_for_history(
        &handle.events,
        |m| matches!(m, OutboundMessage::AssistantMessageEnd(_)),
        "AssistantMessageEnd",
    )
    .await;

    assert_reattach_replays_completed_turn(&handle);

    // The run persists across turns (one run_id per session
    // lifetime): it is still registered and can run another turn.
    assert!(registry.contains_key("run-1"), "run stays registered");

    assert_stop_deregisters(&handle, &registry).await;
}

/// Reattach: a brand-new subscribe replays the whole session,
/// including the now-completed turn. This is the core
/// reattachability guarantee — a dropped+reconnected socket
/// backfills the turn it missed.
fn assert_reattach_replays_completed_turn(handle: &ChatRunHandle) {
    let reattach = handle.events.subscribe();
    assert!(
        reattach
            .history
            .iter()
            .any(|m| matches!(m, OutboundMessage::SessionReady(_))),
        "reattach replays SessionReady"
    );
    assert!(
        reattach
            .history
            .iter()
            .any(|m| matches!(m, OutboundMessage::AssistantMessageEnd(_))),
        "reattach replays the completed turn's AssistantMessageEnd"
    );
}

/// Explicit stop tears the run down and deregisters it.
async fn assert_stop_deregisters(handle: &ChatRunHandle, registry: &ChatRunRegistry) {
    handle.shutdown.cancel();
    for _ in 0..200 {
        if !registry.contains_key("run-1") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !registry.contains_key("run-1"),
        "explicit stop deregisters the run"
    );
    assert!(
        handle.events.subscribe().already_done,
        "stopped run marks its channel done"
    );
}
