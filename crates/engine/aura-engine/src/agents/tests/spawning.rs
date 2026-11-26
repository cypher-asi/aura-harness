use super::*;

#[tokio::test]
async fn task_children_spawn_their_own_children() {
    let h = harness(AgentHostConfig::default());

    let result = h
        .hooks
        .dispatch(task_request(h.root, "level 3"))
        .await
        .unwrap();

    assert_eq!(result.exit, SubagentExit::Completed, "{result:?}");
    assert!(result.final_message.contains("leaf"), "{result:?}");
    assert!(
        result.final_message.matches("finished").count() >= 2,
        "level 3 and level 2 should both have called task: {result:?}"
    );
    assert_eq!(
        h.host.tree(h.root).len(),
        1,
        "finished task children are removed"
    );
}

#[tokio::test]
async fn observer_sees_the_spawn_record_and_the_childs_entries() {
    let seen = Arc::new(SeenEntries::default());
    let h = harness_observed(AgentHostConfig::default(), Some(seen.clone()));

    let result = h
        .hooks
        .dispatch(task_request(h.root, "level 1"))
        .await
        .unwrap();
    assert_eq!(result.exit, SubagentExit::Completed, "{result:?}");

    let seen = seen.0.lock();
    let spawn = seen
        .iter()
        .find(|e| e.tx.tx_type == aura_core_types::TransactionType::SubagentSpawn)
        .expect("spawn record should be observed");
    assert_eq!(spawn.tx.agent_id, h.root);
    let child = result.child_agent_id.unwrap();
    assert!(
        seen.iter().any(|e| e.tx.agent_id == child
            && e.tx.tx_type == aura_core_types::TransactionType::Reasoning),
        "the child's turn should be observed"
    );
}

#[tokio::test]
async fn turn_observer_hears_each_turn_in_order() {
    let h = harness(AgentHostConfig::default());
    let seen = Arc::new(SeenTurns::default());
    h.host.set_turn_observer(seen.clone());

    let result = h
        .hooks
        .dispatch(task_request(h.root, "level 1"))
        .await
        .unwrap();
    assert_eq!(result.exit, SubagentExit::Completed, "{result:?}");

    let seen = seen.0.lock().clone();
    assert_eq!(
        seen,
        vec![
            "start level 1 parent=true".to_string(),
            "event".to_string(),
            "end result=true Completed".to_string(),
            "removed".to_string(),
        ]
    );
}

#[tokio::test]
async fn task_child_is_visible_while_it_runs() {
    let h = harness(AgentHostConfig::default());
    let hooks = h.hooks.clone();
    let root = h.root;
    let run = tokio::spawn(async move { hooks.dispatch(task_request(root, "slow")).await });

    let mut seen = None;
    for _ in 0..200 {
        if let Some(child) = h.host.tree(h.root).into_iter().nth(1) {
            seen = Some(child);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let child = seen.expect("task child should be listed");
    assert_eq!(child.depth, 1);
    assert_eq!(child.parent, Some(h.root));

    h.hooks
        .lifecycle(&child.agent_id.to_hex(), None, None, "stop")
        .await
        .unwrap();
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.exit, SubagentExit::Cancelled);
}

#[tokio::test]
async fn spawning_past_the_depth_cap_is_rejected() {
    let h = harness(AgentHostConfig {
        max_depth: 3,
        ..AgentHostConfig::default()
    });
    let mut request = task_request(h.root, "level 1");
    request.parent_chain = vec![h.root, AgentId::generate(), AgentId::generate()];

    let result = h.hooks.dispatch(request).await.unwrap();

    assert!(
        matches!(&result.exit, SubagentExit::Rejected { reason } if reason.contains("depth")),
        "{result:?}"
    );
    assert_eq!(h.host.tree(h.root).len(), 1);
}

#[tokio::test]
async fn children_cannot_widen_permissions() {
    let h = harness(AgentHostConfig::default());
    let mut request = task_request(h.root, "level 1");
    request.parent_permissions = AgentPermissions {
        capabilities: vec![Capability::SpawnAgent],
        ..AgentPermissions::empty()
    };
    request.override_permissions = Some(AgentPermissions {
        capabilities: vec![Capability::SpawnAgent, Capability::InvokeProcess],
        ..AgentPermissions::empty()
    });

    let result = h.hooks.dispatch(request).await.unwrap();

    assert!(
        matches!(result.exit, SubagentExit::Rejected { .. }),
        "{result:?}"
    );
}

#[tokio::test]
async fn detached_task_result_reaches_the_parent() {
    let mut h = harness(AgentHostConfig::default());
    let mut request = task_request(h.root, "level 1");
    request.spawn_mode = Some(SpawnMode::Detached);

    let ack = h.hooks.dispatch(request).await.unwrap();
    let child = ack.child_agent_id.expect("child id");

    let message = next_message(&mut h.inbox).await;
    assert_eq!(message.to, h.root);
    assert_eq!(message.from, child);
    assert_eq!(message.kind, MessageKind::Reply);
    assert_eq!(message.text, "leaf");
}
