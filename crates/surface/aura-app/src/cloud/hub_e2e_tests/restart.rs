//! A machine works on while no hub watches it (Aura quit or crashed):
//! the next hub follows the turn it was asked for to its end, and copies
//! what the machine made meanwhile.

use super::*;
use aura_core_types::time::now_ms;
use aura_engine_cloud::hub::{TurnAsk, TurnMark};

/// Long enough that the turn still runs when its hub goes.
const MODEL_LATENCY_MS: u64 = 1_500;
/// How long a followed turn may take to end.
const PATIENCE: Duration = Duration::from_secs(30);

fn ask() -> TurnAsk {
    TurnAsk {
        prompt: "fix the build".into(),
        model: Some("mock-model".into()),
        ..Default::default()
    }
}

/// Start a turn on `p`'s machine and stop the hub once the prompt went
/// out, as Aura quitting does. Returns the turn's mark.
async fn left_running(p: &Placed) -> TurnMark {
    let mut events = p.hub.run_turn(p.agent, ask(), CancellationToken::new());
    loop {
        match events.recv().await.expect("the turn runs") {
            RemoteEvent::Status(status) if status == "Working in the cloud" => break,
            RemoteEvent::Failed(err) => panic!("the turn failed: {err}"),
            _ => {}
        }
    }
    p.hub.shutdown();
    while let Some(event) = events.recv().await {
        assert!(
            !matches!(event, RemoteEvent::Done(_) | RemoteEvent::Failed(_)),
            "the hub let go of the turn as it ran: {event:?}"
        );
    }
    p.hub
        .placed_agent(p.agent)
        .and_then(|placed| placed.turn)
        .expect("the turn is marked")
}

/// How the turn `events` follows ended; it never streams its reply.
async fn ending(events: &mut tokio::sync::mpsc::Receiver<RemoteEvent>) -> RemoteEvent {
    let followed = async {
        let mut end = None;
        while let Some(event) = events.recv().await {
            match event {
                RemoteEvent::Text(_) | RemoteEvent::ToolStart { .. } => {
                    panic!("a followed turn's chat comes from its record: {event:?}")
                }
                event @ (RemoteEvent::Done(_) | RemoteEvent::Failed(_)) => end = Some(event),
                _ => {}
            }
        }
        end
    };
    tokio::time::timeout(PATIENCE, followed)
        .await
        .expect("the followed turn ends")
        .expect("with an end")
}

/// Whether `store` holds a reply of `agent` after `head`.
fn replied_after(store: &dyn Store, agent: AgentId, head: u64) -> bool {
    store
        .scan_record(agent, head + 1, 1_000)
        .expect("scan")
        .iter()
        .any(|entry| entry.tx.tx_type == TransactionType::AgentMsg)
}

#[tokio::test]
async fn a_turn_left_running_is_followed_to_its_end_by_the_next_hub() {
    let p = placed_slow(MODEL_LATENCY_MS).await;
    p.import().await;
    let mark = left_running(&p).await;
    let saved = std::fs::read_to_string(p.hub_dir.path().join("cloud/placements.json"))
        .expect("placements");
    assert!(saved.contains(&mark.run_id), "the mark outlives the hub");
    assert_eq!(mark.label, "fix the build");

    let hub = p.restarted().await;
    assert_eq!(hub.unfinished_turns(), vec![(p.agent, mark.clone())]);
    let mut events = hub.follow_turn(p.agent, mark.clone(), CancellationToken::new());

    let end = ending(&mut events).await;
    assert!(matches!(end, RemoteEvent::Done(_)), "{end:?}");
    assert!(
        hub.unfinished_turns().is_empty(),
        "its mark goes once it ended"
    );
    assert!(
        replied_after(hooks::store(&hub).as_ref(), p.agent, mark.head),
        "its reply was copied here"
    );
    hub.shutdown();
}

#[tokio::test]
async fn a_turn_that_ended_while_no_hub_watched_ends_as_its_record_says() {
    let p = placed_slow(MODEL_LATENCY_MS).await;
    p.import().await;
    let node = hooks::node(&p.hub, &p.agent.to_hex()).expect("node");
    let mark = left_running(&p).await;
    let mut alone = false;
    for _ in 0..200 {
        alone = replied_after(p.node.store.as_ref(), p.agent, mark.head);
        if alone {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(alone, "the node finished the turn on its own");
    node.stop_run(&mark.run_id).await.expect("its run let go");

    let hub = p.restarted().await;
    let mut events = hub.follow_turn(p.agent, mark.clone(), CancellationToken::new());

    let RemoteEvent::Done(usage) = ending(&mut events).await else {
        panic!("it replied, so it is done");
    };
    assert_eq!(
        *usage,
        aura_core_protocol::SessionUsage::default(),
        "no stream told its usage"
    );
    assert!(hub.unfinished_turns().is_empty());
    assert!(replied_after(
        hooks::store(&hub).as_ref(),
        p.agent,
        mark.head
    ));
    hub.shutdown();
}

#[tokio::test]
async fn what_a_machine_made_while_no_hub_watched_is_copied_when_the_next_starts() {
    let p = placed(CloudConfig::defaults()).await;
    p.hub.shutdown();
    p.write_on_node(1, 3);

    let hub = p.restarted().await;

    assert_eq!(hooks::catch_up_due(&hub), vec![p.agent.to_hex()]);
    assert!(p.copied_up_to(3).await, "copied with no turn to follow");
    hub.shutdown();
}

#[tokio::test]
async fn a_machine_put_to_sleep_with_all_of_it_copied_is_left_asleep_by_the_next_hub() {
    let p = placed(CloudConfig::defaults()).await;
    repo(&p.node_dir.path().join("ws"), "one\n");
    p.write_on_node(1, 2);
    p.hub.sleep(p.agent).await.expect("slept");
    assert!(
        p.hub
            .placed_agent(p.agent)
            .and_then(|placed| placed.slept_ms)
            .is_some(),
        "all of it was copied before it slept"
    );
    p.hub.shutdown();

    let hub = p.restarted().await;
    assert!(hooks::catch_up_due(&hub).is_empty(), "nothing new on it");
    hub.shutdown();

    let mut reached = p.fake.machine(MACHINE).expect("machine");
    reached.last_connected_ms = now_ms() + 60_000;
    p.fake.insert_machine(reached);
    let hub = p.restarted().await;
    assert_eq!(
        hooks::catch_up_due(&hub),
        vec![p.agent.to_hex()],
        "something reached it after it slept"
    );
    hub.shutdown();
}

#[tokio::test]
async fn a_turn_whose_stream_dropped_is_followed_once_and_ends_once_across_reopens() {
    let p = placed_slow(MODEL_LATENCY_MS).await;
    p.import().await;
    // Its stream drops mid-turn as Aura quits.
    let mark = left_running(&p).await;

    let hub = p.restarted().await;
    assert_eq!(
        hub.unfinished_turns(),
        vec![(p.agent, mark.clone())],
        "one turn to follow"
    );
    let mut events = hub.follow_turn(p.agent, mark, CancellationToken::new());
    let ends = tokio::time::timeout(PATIENCE, async {
        let mut ends = Vec::new();
        while let Some(event) = events.recv().await {
            if matches!(event, RemoteEvent::Done(_) | RemoteEvent::Failed(_)) {
                ends.push(event);
            }
        }
        ends
    })
    .await
    .expect("the followed turn ends");
    assert!(
        matches!(ends.as_slice(), [RemoteEvent::Done(_)]),
        "one end: {ends:?}"
    );
    hub.shutdown();

    // Reopened again: nothing is left to follow, so nothing ends twice.
    let again = p.restarted().await;
    assert!(again.unfinished_turns().is_empty());
    again.shutdown();
}
