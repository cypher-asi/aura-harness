use super::*;
use dashmap::DashMap;
use std::time::Duration;

/// Test double: records the streamed event channel and replays a
/// canned [`AgentLoopEvent`] before returning the configured exit.
struct StubDispatch {
    exit: SubagentExit,
    streamed_text: Option<String>,
}

#[async_trait]
impl EventAwareSubagentDispatch for StubDispatch {
    async fn dispatch_with_events(
        &self,
        _request: SubagentDispatchRequest,
        event_tx: Option<mpsc::Sender<AgentLoopEvent>>,
        _cancellation: Option<CancellationToken>,
    ) -> Result<SubagentResult, String> {
        if let (Some(tx), Some(text)) = (event_tx.as_ref(), self.streamed_text.as_ref()) {
            let _ = tx.send(AgentLoopEvent::TextDelta(text.clone())).await;
        }
        Ok(SubagentResult {
            child_agent_id: None,
            final_message: "done".into(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            files_changed: Vec::new(),
            exit: self.exit.clone(),
        })
    }
}

fn request() -> SubagentDispatchRequest {
    SubagentDispatchRequest {
        parent_agent_id: aura_core_types::AgentId::generate(),
        subagent_type: "explore".into(),
        prompt: "investigate".into(),
        originating_user_id: None,
        parent_chain: Vec::new(),
        reasoning_effort_override: None,
        model_override: None,
        system_prompt_addendum: None,
        parent_permissions: aura_core_types::AgentPermissions::empty(),
        parent_tool_permissions: None,
        user_tool_defaults: aura_core_types::UserToolDefaults::full_access(),
        tool_call_id: Some("toolu_parent_1".into()),
        parent_mode: None,
        parent_kernel_mode: None,
        parent_model_id: None,
        override_mode: None,
        override_permissions: None,
        override_tool_subset: None,
        override_isolation_id: None,
        override_budget: None,
        spawn_mode: None,
        council_index: None,
        council_parent_tool_use_id: None,
        check: None,
        todo_id: None,
        description: None,
    }
}

fn hook_with(
    stub: StubDispatch,
) -> (
    RuntimeSubagentObservabilityHook,
    mpsc::Receiver<OutboundMessage>,
    ChatRunRegistry,
) {
    let (parent_tx, parent_rx) = mpsc::channel::<OutboundMessage>(16);
    let registry: ChatRunRegistry = Arc::new(DashMap::new());
    let hook = RuntimeSubagentObservabilityHook::new(
        Arc::new(stub),
        parent_tx,
        registry.clone(),
        Some(CancellationToken::new()),
        None,
    );
    (hook, parent_rx, registry)
}

/// A successful dispatch emits `SubagentSpawned` (with the parent
/// tool-use id wired through) followed by `SubagentStatus`
/// (`completed`) on the parent stream, registers the minted child
/// run id, and the child stream replays the streamed text frame so
/// an attaching client receives live events.
#[tokio::test]
async fn spawn_then_completed_status_and_child_stream_is_attachable() {
    let (hook, mut parent_rx, registry) = hook_with(StubDispatch {
        exit: SubagentExit::Completed,
        streamed_text: Some("hello from child".into()),
    });

    let result = hook.dispatch(request()).await.expect("dispatch ok");
    assert_eq!(result.final_message, "done");

    // Parent stream: SubagentSpawned first, then SubagentStatus.
    let child_run_id = match parent_rx.recv().await.expect("spawned frame") {
        OutboundMessage::SubagentSpawned(spawned) => {
            assert_eq!(spawned.subagent_type, "explore");
            assert_eq!(spawned.prompt, "investigate");
            assert_eq!(
                spawned.parent_tool_use_id.as_deref(),
                Some("toolu_parent_1")
            );
            spawned.child_run_id
        }
        other => panic!("expected SubagentSpawned, got {other:?}"),
    };
    match parent_rx.recv().await.expect("status frame") {
        OutboundMessage::SubagentStatus(status) => {
            assert_eq!(status.child_run_id, child_run_id);
            assert_eq!(status.state, "completed");
            assert!(status.reason.is_none());
        }
        other => panic!("expected SubagentStatus, got {other:?}"),
    }

    // The child run id is attachable via the chat-run registry.
    let handle = registry.get(&child_run_id).expect("child run registered");
    let events = handle.events.clone();
    drop(handle);

    assert_child_stream_replays_text(&events).await;
}

/// The child stream replays the streamed text frame (forwarded
/// from the child loop's AgentLoopEvent) plus the terminal status.
async fn assert_child_stream_replays_text(events: &ChatEventChannel) {
    let mut saw_text = false;
    for _ in 0..200 {
        let sub = events.subscribe();
        saw_text = sub.history.iter().any(
            |m| matches!(m, OutboundMessage::TextDelta(delta) if delta.text == "hello from child"),
        );
        if saw_text {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        saw_text,
        "child stream should replay the forwarded text frame"
    );
    assert!(
        events
            .subscribe()
            .history
            .iter()
            .any(|m| matches!(m, OutboundMessage::SubagentStatus(_))),
        "child stream should carry the terminal status"
    );
    assert!(events.subscribe().already_done, "child stream marked done");
}

/// A council-member dispatch (`council_index` set) stamps the
/// configured combine mechanism onto the emitted `SubagentSpawned`,
/// while an ordinary `task` spawn leaves it `None` even when the hook
/// carries a mechanism.
#[tokio::test]
async fn council_member_spawn_carries_mechanism() {
    let (parent_tx, mut parent_rx) = mpsc::channel::<OutboundMessage>(16);
    let registry: ChatRunRegistry = Arc::new(DashMap::new());
    let hook = RuntimeSubagentObservabilityHook::new(
        Arc::new(StubDispatch {
            exit: SubagentExit::Completed,
            streamed_text: None,
        }),
        parent_tx,
        registry,
        Some(CancellationToken::new()),
        None,
    )
    .with_council_mechanism(Some("side_by_side".to_string()));

    let mut council_req = request();
    council_req.council_index = Some(0);
    council_req.council_parent_tool_use_id = Some("council-1".to_string());
    let _ = hook.dispatch(council_req).await.expect("dispatch ok");

    match parent_rx.recv().await.expect("spawned frame") {
        OutboundMessage::SubagentSpawned(spawned) => {
            assert_eq!(spawned.council_index, Some(0));
            assert_eq!(spawned.council_mechanism.as_deref(), Some("side_by_side"));
        }
        other => panic!("expected SubagentSpawned, got {other:?}"),
    }
}

/// An ordinary `task` spawn never carries a council mechanism even
/// when the hook was (unexpectedly) configured with one.
#[tokio::test]
async fn task_spawn_omits_council_mechanism() {
    let (parent_tx, mut parent_rx) = mpsc::channel::<OutboundMessage>(16);
    let registry: ChatRunRegistry = Arc::new(DashMap::new());
    let hook = RuntimeSubagentObservabilityHook::new(
        Arc::new(StubDispatch {
            exit: SubagentExit::Completed,
            streamed_text: None,
        }),
        parent_tx,
        registry,
        Some(CancellationToken::new()),
        None,
    )
    .with_council_mechanism(Some("contrast".to_string()));

    let _ = hook.dispatch(request()).await.expect("dispatch ok");

    match parent_rx.recv().await.expect("spawned frame") {
        OutboundMessage::SubagentSpawned(spawned) => {
            assert!(spawned.council_index.is_none());
            assert!(spawned.council_mechanism.is_none());
        }
        other => panic!("expected SubagentSpawned, got {other:?}"),
    }
}

/// A rejected dispatch (e.g. depth/quota) surfaces as
/// `SubagentStatus { state: "rejected", reason: Some(..) }` on the
/// parent stream.
#[tokio::test]
async fn rejected_dispatch_reports_reason_on_parent_stream() {
    let (hook, mut parent_rx, _registry) = hook_with(StubDispatch {
        exit: SubagentExit::Rejected {
            reason: "depth exceeded".into(),
        },
        streamed_text: None,
    });

    let _ = hook.dispatch(request()).await.expect("dispatch ok");

    assert!(matches!(
        parent_rx.recv().await,
        Some(OutboundMessage::SubagentSpawned(_))
    ));
    match parent_rx.recv().await.expect("status frame") {
        OutboundMessage::SubagentStatus(status) => {
            assert_eq!(status.state, "rejected");
            assert_eq!(status.reason.as_deref(), Some("depth exceeded"));
        }
        other => panic!("expected SubagentStatus, got {other:?}"),
    }
}

/// The child run is registered in the SHARED chat-run registry
/// (same `register_run` path as a top-level `POST /v1/run`) carrying
/// parent linkage metadata, and is reaped from the registry after
/// the (here-shortened) retention window once the run completes.
#[tokio::test]
async fn child_run_registered_with_linkage_and_reaped_on_completion() {
    let (parent_tx, _parent_rx) = mpsc::channel::<OutboundMessage>(16);
    let registry: ChatRunRegistry = Arc::new(DashMap::new());
    let parent_cancel = CancellationToken::new();
    let hook = RuntimeSubagentObservabilityHook::new(
        Arc::new(StubDispatch {
            exit: SubagentExit::Completed,
            streamed_text: None,
        }),
        parent_tx,
        registry.clone(),
        Some(parent_cancel.clone()),
        Some("run-parent".to_string()),
    )
    .with_cleanup_retention(Duration::from_millis(150));

    // Request with a populated ancestor chain + spawning tool-use id
    // so every linkage field is exercised.
    let ancestor_a = aura_core_types::AgentId::generate();
    let ancestor_b = aura_core_types::AgentId::generate();
    let mut req = request();
    req.parent_chain = vec![ancestor_a, ancestor_b];
    req.tool_call_id = Some("toolu_linkage".to_string());

    let _ = hook.dispatch(req).await.expect("dispatch ok");

    // Exactly one child run is registered; capture its key + handle.
    let (child_run_id, linkage) = {
        assert_eq!(registry.len(), 1, "one child run registered");
        let entry = registry.iter().next().expect("child run present");
        let linkage = entry
            .value()
            .linkage
            .clone()
            .expect("child run carries parent linkage");
        (entry.key().clone(), linkage)
    };

    assert_eq!(linkage.parent_run_id.as_deref(), Some("run-parent"));
    assert_eq!(linkage.parent_tool_use_id.as_deref(), Some("toolu_linkage"));
    assert_eq!(linkage.child_run_id, child_run_id);
    assert_eq!(linkage.depth, 2);
    assert_eq!(
        linkage.parent_chain,
        vec![ancestor_a.to_string(), ancestor_b.to_string()]
    );

    // Reaped after the retention window: the completed child does
    // not leak in the shared registry.
    for _ in 0..100 {
        if !registry.contains_key(&child_run_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !registry.contains_key(&child_run_id),
        "completed child run reaped from the shared registry"
    );
}

/// A detached dispatch must reflect `running` immediately (not a
/// premature `completed` from the ack), then emit a terminal
/// `completed` once the child's event stream closes.
#[tokio::test]
async fn detached_dispatch_reports_running_then_completed() {
    let (hook, mut parent_rx, _registry) = hook_with(StubDispatch {
        exit: SubagentExit::Completed,
        streamed_text: Some("working".into()),
    });

    let mut detached = request();
    detached.spawn_mode = Some(aura_core_types::SpawnMode::Detached);
    let _ = hook.dispatch(detached).await.expect("dispatch ok");

    assert!(matches!(
        parent_rx.recv().await,
        Some(OutboundMessage::SubagentSpawned(_))
    ));
    match parent_rx.recv().await.expect("running status frame") {
        OutboundMessage::SubagentStatus(status) => {
            assert_eq!(status.state, "running");
            assert!(status.reason.is_none());
        }
        other => panic!("expected running SubagentStatus, got {other:?}"),
    }
    // The completion watch fires once the forwarder drains (the stub
    // dropped its event sender after returning the ack).
    match parent_rx.recv().await.expect("terminal status frame") {
        OutboundMessage::SubagentStatus(status) => {
            assert_eq!(status.state, "completed");
        }
        other => panic!("expected completed SubagentStatus, got {other:?}"),
    }
}
