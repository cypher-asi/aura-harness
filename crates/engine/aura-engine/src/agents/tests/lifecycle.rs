use super::*;

#[tokio::test]
async fn spawned_agent_keeps_its_conversation() {
    let mut h = harness(AgentHostConfig::default());

    let outcome = h
        .hooks
        .spawn_child(&h.root, None, helper_spec("helper", Some("remember 42")))
        .await
        .unwrap();
    let first = next_message(&mut h.inbox).await;
    assert_eq!(first.from, outcome.child_agent_id);
    assert_eq!(first.text, "ok");

    h.hooks
        .deliver_message("helper", None, None, None, "what number?", None, None)
        .await
        .unwrap();
    let second = next_message(&mut h.inbox).await;
    let count: usize = second
        .text
        .strip_prefix("history=")
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("unexpected reply {:?}", second.text));
    assert!(count >= 3, "second turn should see the first: {count}");

    let info = h.host.info(outcome.child_agent_id).unwrap();
    assert_eq!(info.state, AgentState::Idle);
    assert_eq!(info.turns, 2);
}

#[tokio::test]
async fn stopping_an_agent_cancels_it_and_rejects_messages() {
    let h = harness(AgentHostConfig::default());
    let outcome = h
        .hooks
        .spawn_child(&h.root, None, helper_spec("sleeper", Some("slow")))
        .await
        .unwrap();
    let child = outcome.child_agent_id;
    for _ in 0..200 {
        if h.host.info(child).unwrap().state == AgentState::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    h.hooks
        .lifecycle(&child.to_hex(), None, None, "stop")
        .await
        .unwrap();

    assert!(h.host.info(child).is_none());
    let err = h
        .hooks
        .deliver_message("sleeper", None, None, None, "hello", None, None)
        .await
        .unwrap_err();
    assert!(err.contains("unknown agent"), "{err}");
}

#[tokio::test]
async fn list_and_snapshot_see_the_callers_tree() {
    let h = harness(AgentHostConfig::default());
    let outcome = h
        .hooks
        .spawn_child(&h.root, None, helper_spec("helper", None))
        .await
        .unwrap();

    let listed = h.hooks.list_agents(None).await.unwrap();
    let names: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    assert_eq!(names, vec!["root", "helper"]);

    let prefix = &outcome.child_agent_id.to_hex()[..12];
    let snapshot = h.hooks.snapshot(prefix).await.unwrap();
    assert_eq!(snapshot["name"], "helper");
    assert_eq!(snapshot["state"], "idle");

    let stranger = h.host.hooks_for(AgentId::generate());
    assert!(stranger.snapshot("helper").await.is_err());
}
