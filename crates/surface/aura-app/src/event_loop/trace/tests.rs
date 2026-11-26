use super::*;
use aura_core_types::trace::StepAction;

#[test]
fn latest_bundle_matches_the_agent_by_id_or_terminal_task() {
    let home = tempfile::tempdir().unwrap();
    let start = |agent_id: Option<&str>, task: Option<&str>, title: &str| {
        let mut meta = RunMeta::new("m", RunSource::Tui);
        meta.agent_id = agent_id.map(str::to_string);
        meta.task_id = task.map(str::to_string);
        meta.title = title.into();
        let rec = RunRecorder::start(home.path(), meta, aura_config::trace::TraceLevel::Decisions)
            .unwrap()
            .unwrap();
        rec.finish();
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    start(None, Some("tui:aa"), "old");
    start(Some("aa"), None, "new");
    start(Some("bb"), None, "other");

    let found = latest_bundle_of(home.path(), "aa").unwrap();
    assert_eq!(found.read_meta().unwrap().title, "new");
    assert!(latest_bundle_of(home.path(), "cc").is_none());
}

#[tokio::test]
async fn hosted_turns_are_recorded_and_shown_after_a_restart() {
    let home = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let rocks = aura_store_db::RocksStore::open(db.path(), false).unwrap();
    let store = Arc::new(TraceStore::for_store(&rocks));
    let (tx, mut rx) = mpsc::channel(64);
    let traces = Traces::new(
        Some(home.path().to_path_buf()),
        Some(Arc::clone(&store)),
        None,
        tx.clone(),
    );
    let (parent, child) = (AgentId::generate(), AgentId::generate());
    let parent_run = traces
        .start(parent, RunMeta::new("m", RunSource::Tui))
        .unwrap()
        .run_id();

    traces.turn_started(&HostedTurn {
        agent_id: child,
        parent: Some(parent),
        name: "helper",
        model: "m",
        prompt: "count files",
    });
    let child_run = traces.run_id_of(child).unwrap();
    traces.agent_removed(child);
    assert!(traces.recorder(child).is_some(), "kept until its turn ends");
    traces.turn_ended(
        child,
        Some(&AgentLoopResult::default()),
        &SubagentExit::Completed,
    );
    assert!(traces.recorder(child).is_none());
    traces.close_all();
    while rx.try_recv().is_ok() {}

    let stored = db::load(&store, &child_run).unwrap().unwrap();
    assert_eq!(stored.meta.spawned_by.as_deref(), Some(parent_run.as_str()));
    assert_eq!(stored.meta.title, "count files");

    let restarted = Traces::new(Some(home.path().to_path_buf()), Some(store), None, tx);
    restarted.send_history(child).await;
    match rx.try_recv().unwrap() {
        UiCommand::ForAgent { agent_id, command } => {
            assert_eq!(agent_id, child.to_hex());
            assert!(
                matches!(*command, UiCommand::GraphUpsert { run_id, .. } if run_id == child_run)
            );
        }
        other => panic!("unexpected {other:?}"),
    }
    restarted.send_history(child).await;
    assert!(rx.try_recv().is_err(), "history is sent once");
}

/// A model step whose `task` call, `t1`, has `input`.
fn task_call(input: serde_json::Value) -> aura_core_types::trace::Step {
    use aura_core_types::trace::{
        Actor, ModelAction, Observation, PolicyRef, Step, StepLinks, ToolCallAction, ToolIntent,
    };
    Step {
        run_id: String::new(),
        seq: 1,
        parent: None,
        turn: 1,
        iteration: Some(0),
        at_ms: 0,
        actor: Actor::Model,
        observation: Observation::None,
        action: StepAction::Model(ModelAction {
            tool_calls: vec![ToolCallAction {
                tool_use_id: "t1".into(),
                name: "task".into(),
                intent: ToolIntent::classify("task", &input),
                input,
            }],
            ..ModelAction::default()
        }),
        policy: PolicyRef::default(),
        links: StepLinks::default(),
        usage: None,
        duration_ms: None,
    }
}

#[test]
fn a_spawned_run_is_linked_to_the_call_in_the_parents_live_graph() {
    let home = tempfile::tempdir().unwrap();
    let (tx, mut rx) = mpsc::channel(64);
    let traces = Traces::new(Some(home.path().to_path_buf()), None, None, tx);
    let (parent, child) = (AgentId::generate(), AgentId::generate());
    traces
        .start(parent, RunMeta::new("m", RunSource::Tui))
        .unwrap();
    let (_, graph) = traces.live_graph(parent).unwrap();
    let input = serde_json::json!({"prompt": "count files"});
    lock(&graph).apply(&task_call(input));
    while rx.try_recv().is_ok() {}

    traces.turn_started(&HostedTurn {
        agent_id: child,
        parent: Some(parent),
        name: "helper",
        model: "m",
        prompt: "count files",
    });
    let child_run = traces.run_id_of(child).unwrap();
    let call = lock(&graph).node("tc:t1").unwrap().meta["child_run"].clone();
    assert_eq!(call, serde_json::json!(child_run));
    let sent = std::iter::from_fn(|| rx.try_recv().ok()).any(|c| match c {
        UiCommand::ForAgent { agent_id, command } => {
            agent_id == parent.to_hex()
                && matches!(*command, UiCommand::GraphUpsert { nodes, .. }
                    if nodes.iter().any(|n| n.id == "tc:t1"
                        && n.meta.iter().any(|(k, v)| k == "child_run" && *v == child_run)))
        }
        _ => false,
    });
    assert!(sent, "the parent's graph view hears about the link");
    traces.close_all();
}
