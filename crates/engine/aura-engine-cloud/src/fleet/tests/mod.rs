use super::*;

use crate::boot::{BootEvent, StageState};

use crate::fake::FakeProvider;

use aura_core_types::VmUsage;

fn settings(idle: Duration) -> FleetSettings {
    FleetSettings {
        install: "inst-1".into(),
        size: MachineSize::S4x16,
        isolated: true,
        idle_hibernate: idle,
        idle_standby: Duration::ZERO,
    }
}

fn fleet(idle: Duration) -> (Arc<VmFleet>, FakeProvider, Arc<MemoryLedger>) {
    let fake = FakeProvider::new();
    fake.on_exec(Arc::new(|machine, command, stdin| {
        assert_eq!(command, CLAIM_COMMAND);
        let claim: NodeClaim =
            serde_json::from_slice(stdin.unwrap_or_default()).expect("claim json");
        assert_eq!(claim.node_id, machine);
        assert_eq!(claim.node_token.len(), 64);
        let answer = NodeClaimed {
            node_id: claim.node_id,
            port: 8080,
            sealed: claim.seal.len(),
        };
        crate::ExecOutput {
            exit_code: 0,
            stdout: serde_json::to_vec(&answer).expect("json"),
            stderr: Vec::new(),
        }
    }));
    let ledger = Arc::new(MemoryLedger::default());
    let fleet = Arc::new(VmFleet::new(
        Arc::new(fake.clone()),
        ledger.clone(),
        settings(idle),
    ));
    (fleet, fake, ledger)
}

fn request(agent: &str, origin: MachineOrigin) -> ProvisionRequest {
    ProvisionRequest {
        agent: agent.into(),
        agent_zns: format!("0://terminal/{agent}"),
        code: Some("RE36".into()),
        project: "p1".into(),
        name: format!("aura-re36-{agent}"),
        origin,
        seal: Vec::new(),
        owner: ALICE.into(),
        secrets: Vec::new(),
        plans: Vec::new(),
        repo_secret: None,
        git: None,
    }
}

const ALICE: &str = "a11ce000";

const BOB: &str = "b0b00000";

fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<BootEvent>) -> Vec<BootEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

fn stage_trail(events: &[BootEvent]) -> Vec<(BootStage, &'static str)> {
    events
        .iter()
        .filter_map(|event| match event {
            BootEvent::Stage { stage, state } => Some((
                *stage,
                match state {
                    StageState::Started => "started",
                    StageState::Done { .. } => "done",
                    StageState::Skipped(_) => "skipped",
                    StageState::Failed(_) => "failed",
                },
            )),
            _ => None,
        })
        .collect()
}

fn fact<'a>(events: &'a [BootEvent], wanted: &str) -> Option<&'a str> {
    events.iter().rev().find_map(|event| match event {
        BootEvent::Fact { key, value } if key == wanted => Some(value.as_str()),
        _ => None,
    })
}

mod claims;
mod machines;
mod provisioning;
