use super::*;

/// §2 + §8: starting and stopping an automaton must each produce
/// one `System::AutomatonLifecycle` entry in the record log for the
/// owning agent. This test exercises the bridge's
/// `record_lifecycle_event` seam directly so the assertion is
/// focused on the inbox → scheduler → record-log hop that the
/// automaton runtime triggers, without spinning up a real dev loop.
#[tokio::test]
async fn start_then_stop_records_two_automaton_lifecycle_entries() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("noop"));
    let ws_dir = dir.path().join("workspaces");
    std::fs::create_dir_all(&ws_dir).unwrap();

    let scheduler = Arc::new(Scheduler::new(
        store.clone(),
        provider.clone(),
        vec![],
        vec![],
        ws_dir,
        None,
    ));

    let runtime = Arc::new(AutomatonRuntime::new());
    let catalog = Arc::new(ToolCatalog::new());
    let domain: Arc<dyn DomainApi> = Arc::new(UnusedDomain);
    let bridge = AutomatonBridge::new(
        runtime,
        store.clone(),
        domain,
        provider,
        catalog,
        ToolConfig::default(),
    )
    .with_scheduler(scheduler);

    let agent_id = AgentId::generate();

    // The post-fix `Scheduler::schedule_agent` requires the agent's
    // identity to be registered before it will dispatch a tick. The
    // production dev-loop / task-run kickoff paths register identity
    // in `register_automaton_identity` (called from
    // `dispatch.rs::start_dev_loop_with_capabilities`); this test
    // exercises `record_lifecycle_event` directly so we register
    // here to mirror the production flow without spinning up a full
    // dev loop.
    bridge.register_automaton_identity(
        agent_id,
        "claude-test-model",
        None,
        None,
        None,
        None,
        None,
        aura_model::ModelRequestKind::DevLoopBootstrap,
    );

    bridge
        .record_lifecycle_event(agent_id, "aut-1", "start_dev_loop")
        .await;
    bridge
        .record_lifecycle_event(agent_id, "aut-1", "stop_dev_loop")
        .await;

    let count = count_lifecycle_entries(&store, agent_id);
    assert_eq!(
        count, 2,
        "expected exactly 2 System/AutomatonLifecycle entries, got {count}"
    );
}

/// Operator-initiated pause must produce a `pause_dev_loop`
/// `System::AutomatonLifecycle` entry, distinct from `stop_dev_loop`,
/// so the audit log can answer "did the run halt because of a pause
/// or a stop?". Before this contract the pause path only emitted an
/// `info!` line and skipped the inbox write, leaving a gap in the
/// record log between start and stop with no on-record evidence of
/// the user's intentional intervention.
///
/// The follow-up REST-driven scenario (`pause` then `stop` then
/// inspect log) is encoded by running both `record_lifecycle_event`
/// invocations in the order an operator would trigger them; this
/// keeps the test focused on the inbox → scheduler → record-log hop
/// without spinning up a full dev loop, mirroring
/// `start_then_stop_records_two_automaton_lifecycle_entries` above.
#[tokio::test]
async fn pause_then_stop_records_distinct_lifecycle_entries_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("noop"));
    let ws_dir = dir.path().join("workspaces");
    std::fs::create_dir_all(&ws_dir).unwrap();

    let scheduler = Arc::new(Scheduler::new(
        store.clone(),
        provider.clone(),
        vec![],
        vec![],
        ws_dir,
        None,
    ));

    let runtime = Arc::new(AutomatonRuntime::new());
    let catalog = Arc::new(ToolCatalog::new());
    let domain: Arc<dyn DomainApi> = Arc::new(UnusedDomain);
    let bridge = AutomatonBridge::new(
        runtime,
        store.clone(),
        domain,
        provider,
        catalog,
        ToolConfig::default(),
    )
    .with_scheduler(scheduler);

    let agent_id = AgentId::generate();
    bridge.register_automaton_identity(
        agent_id,
        "claude-test-model",
        None,
        None,
        None,
        None,
        None,
        aura_model::ModelRequestKind::DevLoopBootstrap,
    );

    bridge
        .record_lifecycle_event(agent_id, "aut-1", "start_dev_loop")
        .await;
    bridge
        .record_lifecycle_event(agent_id, "aut-1", "pause_dev_loop")
        .await;
    bridge
        .record_lifecycle_event(agent_id, "aut-1", "stop_dev_loop")
        .await;

    let events = lifecycle_events_in_order(&store, agent_id);
    assert_eq!(
        events,
        vec![
            "start_dev_loop".to_string(),
            "pause_dev_loop".to_string(),
            "stop_dev_loop".to_string(),
        ],
        "operator-driven start → pause → stop must land in the record log \
         in that exact order so the audit trail distinguishes intentional \
         pauses from stops",
    );
}
