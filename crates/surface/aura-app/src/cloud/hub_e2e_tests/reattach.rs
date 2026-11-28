//! Reattaching to a turn an earlier hub asked for, as a hub started after
//! the app quit does: the node runs the turn on without a socket, and the
//! new attach replays it, follows it to its end and copies it here.

use super::*;
use crate::cloud::hub::{RUN_GONE, WORKING};
use crate::cloud::test_node::GatedModel;
use aura_model::MockProvider;
use tokio::sync::Semaphore;

/// A placed agent whose node's model waits on the returned gate.
async fn gated() -> (Placed, Arc<Semaphore>) {
    let gate = Arc::new(Semaphore::new(0));
    let node_dir = tempfile::tempdir().expect("tempdir");
    let model = GatedModel {
        reply: MockProvider::simple_response("all done"),
        gate: Arc::clone(&gate),
    };
    let node = TestNode::serve_with(node_dir.path(), Arc::new(model)).await;
    let p = placed_on(
        CloudConfig::defaults(),
        TestNode::exec_handler,
        (node, node_dir),
    )
    .await;
    p.import().await;
    (p, gate)
}

/// Every event `events` sends until it closes.
async fn all_of(events: &mut tokio::sync::mpsc::Receiver<RemoteEvent>) -> Vec<RemoteEvent> {
    let mut seen = Vec::new();
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(20), events.recv()).await {
        seen.push(event);
    }
    seen
}

#[tokio::test]
async fn a_turn_left_running_by_a_quit_is_followed_to_its_end_by_a_new_attach() {
    let (p, gate) = gated().await;
    let before = p.local_head();
    let ask = crate::cloud::hub::TurnAsk {
        prompt: "fix the footer".into(),
        model: Some("mock-model".into()),
        ..Default::default()
    };
    let mut first = p.hub.run_turn(p.agent, ask, CancellationToken::new());
    while let Some(event) = first.recv().await {
        if event == RemoteEvent::Status(WORKING.into()) {
            break;
        }
    }
    let run = p
        .hub
        .turn_run(p.agent)
        .expect("the hub knows the turn's run");
    // The app quits: nothing takes the first stream's events any more,
    // and the node runs the turn on, held at its model call.
    drop(first);

    let mut again = p
        .hub
        .reattach_turn(p.agent, run.clone(), CancellationToken::new());
    gate.add_permits(16);
    let seen = all_of(&mut again).await;
    assert!(
        seen.iter().any(|e| matches!(e, RemoteEvent::Done(_))),
        "{seen:?}"
    );
    assert!(
        !seen.iter().any(|e| matches!(e, RemoteEvent::Failed(_))),
        "{seen:?}"
    );
    let replied: String = seen
        .iter()
        .filter_map(|e| match e {
            RemoteEvent::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(replied.contains("all done"), "{seen:?}");
    assert!(
        p.copied_up_to(before + 2).await,
        "its record is copied here"
    );
    assert_eq!(p.hub.turn_run(p.agent), None, "it is no longer streamed");
}

#[tokio::test]
async fn a_run_its_machine_no_longer_has_ends_the_reattach_as_gone() {
    let (p, _gate) = gated().await;
    let mut again = p
        .hub
        .reattach_turn(p.agent, "no-such-run".into(), CancellationToken::new());
    let seen = all_of(&mut again).await;
    assert_eq!(
        seen.last(),
        Some(&RemoteEvent::Failed(RUN_GONE.into())),
        "{seen:?}"
    );
}
