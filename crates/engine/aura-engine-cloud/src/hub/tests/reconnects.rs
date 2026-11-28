//! A hub that starts while cloud turns run reconnects to them: it claims
//! their nodes again, but never wakes a machine that is up, never leaves a
//! sleeping one awake for nothing, and keeps trying a node it can't reach.

use super::*;
use crate::fleet::CLAIM_COMMAND;
use crate::{BootEvent, BootStage, MachineInfo, MachineStatus};
use aura_core_protocol::cloud::{NodeClaim, NODE_PROTOCOL};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Answer every claim as a current node does, or fail it when `fails`;
/// counts the claims.
fn answer_claims(fake: &FakeProvider, fails: bool) -> Arc<AtomicUsize> {
    let claims = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&claims);
    fake.on_exec(Arc::new(move |_, command, stdin| {
        if command != CLAIM_COMMAND {
            return crate::ExecOutput::default();
        }
        counted.fetch_add(1, Ordering::SeqCst);
        if fails {
            return crate::ExecOutput {
                exit_code: 1,
                stdout: Vec::new(),
                stderr: b"no route to host".to_vec(),
            };
        }
        let claim: NodeClaim = serde_json::from_slice(stdin.unwrap_or_default()).expect("claim");
        let answer = serde_json::json!({
            "node_id": claim.node_id,
            "port": 8080,
            "sealed": 0,
            "protocol": NODE_PROTOCOL,
            "tools": ["git_ship"],
        });
        crate::ExecOutput {
            exit_code: 0,
            stdout: serde_json::to_vec(&answer).expect("json"),
            stderr: Vec::new(),
        }
    }));
    claims
}

/// Place `agent` on machine `name`, which boxd reports as `status`, as a
/// restarted hub takes it back: not yet claimed.
async fn place_on(
    hub: &CloudHub,
    fake: &FakeProvider,
    agent: AgentId,
    name: &str,
    status: MachineStatus,
) {
    fake.insert_machine(MachineInfo {
        id: format!("vm-{name}"),
        name: name.into(),
        status,
        region: String::new(),
        vcpu: 4,
        memory_bytes: 0,
        disk_bytes: 0,
        domain: format!("{name}.boxd.sh"),
        public_ip: String::new(),
        ssh_port: 0,
        isolated: true,
        labels: std::collections::BTreeMap::new(),
        source: None,
        created_ms: 0,
        boot_ms: 0,
        last_connected_ms: now_ms(),
    });
    hub.fleet
        .adopt(&agent.to_hex(), "0://scout", "p1", name, Vec::new(), None)
        .await
        .expect("adopt");
    let placed = PlacedAgent {
        zns: "0://scout".into(),
        name: "scout".into(),
        project: "p1".into(),
        project_dir: std::env::temp_dir(),
        machine: name.into(),
        owner: None,
        owner_name: None,
        branch: None,
        repo: None,
        fetched_tip: None,
        children: Vec::new(),
        turn: None,
        slept_ms: None,
    };
    hub.placements.lock().agents.insert(agent.to_hex(), placed);
}

fn mark(run: &str) -> TurnMark {
    TurnMark {
        run_id: run.into(),
        started_ms: now_ms(),
        head: 0,
        label: "Fix the build".into(),
        model: None,
        titled: false,
    }
}

/// Events of a followed turn up to its reconnect (or its failure).
async fn until_reconnected(rx: &mut mpsc::Receiver<RemoteEvent>) -> Vec<RemoteEvent> {
    let mut seen = Vec::new();
    let got = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = rx.recv().await {
            let done = matches!(&event, RemoteEvent::Status(s) if s == RECONNECTED)
                || matches!(event, RemoteEvent::Failed(_));
            seen.push(event);
            if done {
                return;
            }
        }
    })
    .await;
    assert!(got.is_ok(), "never reconnected: {seen:?}");
    seen
}

fn wake_steps(events: &[RemoteEvent]) -> usize {
    events
        .iter()
        .filter(|event| match event {
            RemoteEvent::Boot(BootEvent::Plan(stages)) => stages.contains(&BootStage::Wake),
            RemoteEvent::Boot(BootEvent::Stage { stage, .. }) => *stage == BootStage::Wake,
            _ => false,
        })
        .count()
}

#[tokio::test]
async fn a_restart_with_turns_running_reconnects_each_without_a_wake() {
    let (hub, fake, _dir) = hub();
    let claims = answer_claims(&fake, false);
    let agents: Vec<AgentId> = (0..3).map(|_| AgentId::generate()).collect();
    for (n, agent) in agents.iter().enumerate() {
        place_on(
            &hub,
            &fake,
            *agent,
            &format!("aura-sc01-{n:04}"),
            MachineStatus::Running,
        )
        .await;
        hub.mark_turn(*agent, mark(&format!("run-{n}")));
    }
    let cancel = CancellationToken::new();
    let mut streams: Vec<_> = hub
        .unfinished_turns()
        .into_iter()
        .map(|(agent, mark)| hub.follow_turn(agent, mark, cancel.clone()))
        .collect();
    for rx in &mut streams {
        let events = until_reconnected(rx).await;
        assert!(
            matches!(events.first(), Some(RemoteEvent::Status(s)) if s == RECONNECTING),
            "says it reconnects first: {events:?}"
        );
        assert_eq!(wake_steps(&events), 0, "no wake step: {events:?}");
        assert!(
            !events.iter().any(|e| matches!(e, RemoteEvent::Failed(_))),
            "{events:?}"
        );
    }
    assert!(
        fake.calls_matching("Wake(").is_empty(),
        "{:?}",
        fake.calls()
    );
    assert_eq!(
        claims.load(Ordering::SeqCst),
        agents.len(),
        "each node claimed once"
    );
    cancel.cancel();
    hub.shutdown();
}

#[tokio::test]
async fn a_sleeping_machine_with_nothing_to_do_stays_asleep_on_restart() {
    let (hub, fake, _dir) = hub();
    let claims = answer_claims(&fake, false);
    let agent = AgentId::generate();
    // No hub put it to sleep, so it may hold what no hub copied; but no
    // work runs on it.
    place_on(
        &hub,
        &fake,
        agent,
        "aura-sc01-0007",
        MachineStatus::Hibernated,
    )
    .await;
    assert_eq!(hub.catch_up_due(), vec![agent.to_hex()]);
    hub.catch_up().await;
    assert!(
        fake.calls_matching("Wake(").is_empty(),
        "{:?}",
        fake.calls()
    );
    assert_eq!(claims.load(Ordering::SeqCst), 0);
    hub.shutdown();
}

#[tokio::test]
async fn an_unreachable_node_is_tried_again_then_the_turn_fails_with_its_mark_kept() {
    let (hub, fake, _dir) = hub();
    let claims = answer_claims(&fake, true);
    hub.set_reconnect_delays(vec![Duration::from_millis(5); 3]);
    let agent = AgentId::generate();
    place_on(&hub, &fake, agent, "aura-sc01-0009", MachineStatus::Running).await;
    hub.mark_turn(agent, mark("run-9"));
    let mut rx = hub.follow_turn(agent, mark("run-9"), CancellationToken::new());
    let events = until_reconnected(&mut rx).await;
    assert!(
        matches!(events.last(), Some(RemoteEvent::Failed(_))),
        "{events:?}"
    );
    assert_eq!(
        claims.load(Ordering::SeqCst),
        4,
        "the first try and three more"
    );
    assert!(
        fake.calls_matching("Wake(").is_empty(),
        "{:?}",
        fake.calls()
    );
    assert!(
        hub.turn_marked(&agent.to_hex()),
        "it may still run: picked up later"
    );
    hub.shutdown();
}

#[tokio::test]
async fn a_marked_turn_on_a_sleeping_machine_wakes_it_to_copy_the_turn() {
    let (hub, fake, _dir) = hub();
    answer_claims(&fake, false);
    let agent = AgentId::generate();
    place_on(
        &hub,
        &fake,
        agent,
        "aura-sc01-0011",
        MachineStatus::Hibernated,
    )
    .await;
    hub.mark_turn(agent, mark("run-11"));
    let mut rx = hub.follow_turn(agent, mark("run-11"), CancellationToken::new());
    let mut events = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = rx.recv().await {
            let stop = matches!(
                &event,
                RemoteEvent::Boot(BootEvent::Stage {
                    stage: BootStage::Claim,
                    ..
                })
            );
            events.push(event);
            if stop {
                return;
            }
        }
    })
    .await;
    // Its turn ended there: the reply still has to be copied.
    assert_eq!(fake.calls_matching("Wake(").len(), 1, "{:?}", fake.calls());
    hub.shutdown();
}
