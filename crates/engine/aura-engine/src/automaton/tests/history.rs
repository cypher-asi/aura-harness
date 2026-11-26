use super::*;

/// A subscriber that joins after every event has been emitted still
/// sees the full sequence via [`EventSubscription::history`].
#[tokio::test]
async fn late_subscriber_sees_replayed_history_after_done() {
    use aura_engine_automaton::AutomatonEvent;

    let bridge = test_bridge();
    let automaton_id = "aut-replay".to_string();
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    bridge.spawn_event_forwarder(automaton_id.clone(), rx);

    tx.send(AutomatonEvent::Started {
        automaton_id: automaton_id.clone(),
    })
    .await
    .unwrap();
    tx.send(AutomatonEvent::TaskStarted {
        task_id: "task-1".into(),
        task_title: "first task".into(),
    })
    .await
    .unwrap();
    tx.send(AutomatonEvent::TaskFailed {
        task_id: "task-1".into(),
        reason: "boom".into(),
    })
    .await
    .unwrap();
    tx.send(AutomatonEvent::Stopped {
        automaton_id: automaton_id.clone(),
        reason: "Failed".into(),
    })
    .await
    .unwrap();
    tx.send(AutomatonEvent::Done).await.unwrap();

    // Wait for the forwarder to observe Done and set `done=true`.
    // The forwarder pushes history before toggling the flag, so
    // once `already_done` is true we know every event is visible.
    let subscription = loop {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let sub = bridge
            .subscribe_events(&automaton_id)
            .expect("channel still in retention window");
        if sub.already_done {
            break sub;
        }
    };

    let kinds: Vec<&'static str> = subscription
        .history
        .iter()
        .map(|e| match e {
            AutomatonEvent::Started { .. } => "started",
            AutomatonEvent::TaskStarted { .. } => "task_started",
            AutomatonEvent::TaskFailed { .. } => "task_failed",
            AutomatonEvent::Stopped { .. } => "stopped",
            AutomatonEvent::Done => "done",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["started", "task_started", "task_failed", "stopped", "done"],
        "late subscriber must see every emitted event in order"
    );
    assert!(subscription.already_done);
}

/// A subscriber that joins mid-stream sees the events emitted so
/// far through `history` and any later events through `live`, in
/// order. This is what would have saved us in the logs the user
/// shared: the WS would observe `Started → TaskFailed → Done`
/// regardless of whether it subscribed 1 ms or 200 ms after
/// `POST /automaton/start` returned.
#[tokio::test]
async fn mid_stream_subscriber_sees_history_then_live_events() {
    use aura_engine_automaton::AutomatonEvent;

    let bridge = test_bridge();
    let automaton_id = "aut-mid".to_string();
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    bridge.spawn_event_forwarder(automaton_id.clone(), rx);

    tx.send(AutomatonEvent::Started {
        automaton_id: automaton_id.clone(),
    })
    .await
    .unwrap();
    tx.send(AutomatonEvent::TaskStarted {
        task_id: "task-1".into(),
        task_title: "first".into(),
    })
    .await
    .unwrap();

    // Let the forwarder drain the two events above into history.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let mut subscription = bridge
        .subscribe_events(&automaton_id)
        .expect("channel present");
    assert!(!subscription.already_done);
    assert_eq!(subscription.history.len(), 2);
    assert!(matches!(
        subscription.history[0],
        AutomatonEvent::Started { .. }
    ));
    assert!(matches!(
        subscription.history[1],
        AutomatonEvent::TaskStarted { .. }
    ));

    // Emit the remainder after subscribe. These should arrive on
    // the live receiver, not in history (history was snapshotted).
    tx.send(AutomatonEvent::TaskCompleted {
        task_id: "task-1".into(),
        summary: "ok".into(),
    })
    .await
    .unwrap();
    tx.send(AutomatonEvent::Done).await.unwrap();

    let first = subscription.live.recv().await.expect("live event");
    assert!(matches!(first, AutomatonEvent::TaskCompleted { .. }));
    let second = subscription.live.recv().await.expect("live event");
    assert!(matches!(second, AutomatonEvent::Done));
}

/// History is capped at [`EVENT_HISTORY_CAPACITY`] so long-lived
/// dev-loop automatons don't grow unbounded. The oldest events
/// are dropped first; this is consistent with how
/// `tokio::sync::broadcast` would have behaved for an early
/// subscriber that fell behind.
#[tokio::test]
async fn history_caps_at_capacity_and_drops_oldest() {
    use aura_engine_automaton::AutomatonEvent;

    let bridge = test_bridge();
    let automaton_id = "aut-cap".to_string();
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    bridge.spawn_event_forwarder(automaton_id.clone(), rx);

    let over = EVENT_HISTORY_CAPACITY + 5;
    for i in 0..over {
        tx.send(AutomatonEvent::LogLine {
            message: format!("line {i}"),
        })
        .await
        .unwrap();
    }

    // Drain.
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let subscription = bridge
        .subscribe_events(&automaton_id)
        .expect("channel present");
    assert_eq!(
        subscription.history.len(),
        EVENT_HISTORY_CAPACITY,
        "history must be capped at EVENT_HISTORY_CAPACITY"
    );
    // The very first 5 "line 0".."line 4" should have been evicted.
    match &subscription.history[0] {
        AutomatonEvent::LogLine { message } => {
            assert_eq!(
                message, "line 5",
                "oldest surviving entry should be the 6th emitted event"
            );
        }
        other => panic!("unexpected event kind: {other:?}"),
    }
}
