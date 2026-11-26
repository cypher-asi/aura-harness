//! When a cloud agent's record is copied here: before its machine
//! sleeps, while a link is open, and what it spawned.

use super::*;

#[tokio::test]
async fn an_idle_machine_is_copied_before_it_sleeps() {
    let p = placed(CloudConfig {
        idle_hibernate_secs: 1,
        ..CloudConfig::defaults()
    })
    .await;
    p.write_on_node(1, 3);
    tokio::time::sleep(Duration::from_millis(1100)).await;

    hooks::sleep_idle_machines(&p.hub).await;

    assert_eq!(p.local_head(), 3, "copied before it slept");
    assert_eq!(
        p.fake.machine(MACHINE).expect("machine").status,
        MachineStatus::Hibernated
    );
}

#[tokio::test]
async fn sleeping_a_machine_by_hand_copies_first() {
    let p = placed(CloudConfig::defaults()).await;
    p.write_on_node(1, 2);

    p.hub.sleep(p.agent).await.expect("slept");

    assert_eq!(p.local_head(), 2);
    assert_eq!(
        p.fake.machine(MACHINE).expect("machine").status,
        MachineStatus::Hibernated
    );
}

#[tokio::test]
async fn an_opened_link_catches_up_then_follows_the_node() {
    let p = placed(CloudConfig::defaults()).await;
    p.import().await;
    p.write_on_node(1, 2);

    hooks::open_link(&p.hub, &p.agent.to_hex());
    assert!(p.copied_up_to(2).await, "caught up when the link opened");

    p.write_on_node(3, 4);
    assert!(
        p.copied_up_to(4).await,
        "the node's hint brought the new entries"
    );
    p.hub.shutdown();
}

#[tokio::test]
async fn hints_while_a_copy_runs_fold_into_one_more() {
    let p = placed(CloudConfig::defaults()).await;
    p.write_on_node(1, 2);
    let gate = hooks::copy_gate(&p.hub, p.agent);
    let held = gate.lock().await;

    for _ in 0..10 {
        hooks::copy_soon(&p.hub, p.agent);
    }
    assert_eq!(
        hooks::copies(&p.hub).get(&p.agent),
        Some(&true),
        "one copy waits, one more is owed"
    );
    drop(held);

    assert!(p.copied_up_to(2).await);
    for _ in 0..100 {
        if hooks::copies(&p.hub).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(hooks::copies(&p.hub).is_empty(), "no copy left running");
}

#[derive(Debug, Default)]
struct Seen(Mutex<Vec<(AgentId, u64)>>);

impl RecordObserver for Seen {
    fn on_entry(&self, entry: &RecordEntry) {
        self.0.lock().push((entry.tx.agent_id, entry.seq));
    }
}

#[tokio::test]
async fn copied_entries_reach_the_observer_in_order() {
    let p = placed(CloudConfig::defaults()).await;
    let seen = Arc::new(Seen::default());
    p.hub
        .attach_observer(Arc::clone(&seen) as Arc<dyn RecordObserver>);
    p.write_on_node(1, 3);
    assert!(!p.hub.take_copied());

    assert_eq!(p.hub.mirror(p.agent).await.expect("copied"), 3);

    assert_eq!(
        *seen.0.lock(),
        vec![(p.agent, 1), (p.agent, 2), (p.agent, 3)]
    );
    assert!(p.hub.take_copied(), "the agent lists are told");
    assert!(!p.hub.take_copied(), "once");
}

#[tokio::test]
async fn spawned_agents_are_copied_only_when_the_record_names_them() {
    let p = placed(CloudConfig::defaults()).await;
    let child = AgentId::new([7; 32]);
    let stranger = AgentId::new([8; 32]);
    let known = AgentId::new([9; 32]);
    append(
        hooks::store(&p.hub).as_ref(),
        1,
        Transaction::user_prompt(known, "mine"),
    );
    append(
        p.node.store.as_ref(),
        1,
        Transaction::user_prompt(p.agent, "split the work"),
    );
    append(p.node.store.as_ref(), 2, spawned(p.agent, child));
    append(p.node.store.as_ref(), 3, spawned(p.agent, known));
    for agent in [child, stranger, known] {
        for seq in 1..=2 {
            append(
                p.node.store.as_ref(),
                seq,
                Transaction::user_prompt(agent, format!("p{seq}")),
            );
        }
    }

    assert_eq!(p.hub.mirror(p.agent).await.expect("copied"), 5);

    assert_eq!(p.head_of(child), 2, "named by the record: copied");
    assert_eq!(p.head_of(stranger), 0, "never named: left on the machine");
    assert_eq!(p.head_of(known), 1, "already has a record here: not taken");
    assert_eq!(p.hub.sync_status(p.agent).expect("placed").children, 1);
    let saved = std::fs::read_to_string(p.hub_dir.path().join("cloud/placements.json"))
        .expect("placements");
    assert!(saved.contains(&child.to_hex()) && !saved.contains(&known.to_hex()));

    append(
        p.node.store.as_ref(),
        3,
        Transaction::user_prompt(child, "p3"),
    );
    assert_eq!(p.hub.mirror(p.agent).await.expect("copied"), 1);
    assert_eq!(p.head_of(child), 3, "and kept up with");
}

#[tokio::test]
async fn a_turn_s_run_is_copied_into_the_main_trace_store() {
    let p = placed(CloudConfig::defaults()).await;
    let db = RocksStore::open(p.hub_dir.path().join("traces"), false).expect("open");
    let traces = Arc::new(TraceStore::for_store(&db));
    p.hub.attach_traces(Arc::clone(&traces));
    p.import().await;

    let mut events = p.hub.run_turn(
        p.agent,
        "hello".into(),
        Some("mock-model".into()),
        None,
        None,
        CancellationToken::new(),
    );
    while let Some(event) = events.recv().await {
        if let RemoteEvent::Failed(err) = event {
            panic!("the turn failed: {err}");
        }
    }

    let run = aura_agent_trace::db::latest_of(&traces, &p.agent.to_hex())
        .expect("read")
        .expect("the turn's run is here");
    assert_eq!(
        run.meta.agent_id.as_deref(),
        Some(p.agent.to_hex().as_str())
    );
    assert!(!run.steps.is_empty(), "with its steps");
    assert_eq!(p.hub.take_traces_copied(), HashSet::from([p.agent]));
    assert!(p.hub.take_traces_copied().is_empty(), "once");
    assert!(p
        .hub
        .sync_status(p.agent)
        .expect("placed")
        .traces_ms
        .is_some());
}
