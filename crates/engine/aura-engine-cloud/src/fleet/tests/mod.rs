use super::*;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use aura_config::MachineSize;
use aura_core_protocol::cloud::{GitClaim, NodeClaim, NodeClaimed, NODE_PROTOCOL};
use aura_core_types::{VmEvent, VmSource};
use parking_lot::Mutex;

use crate::boot::{BootEvent, BootReporter, BootStage, StageState};
use crate::boxd::CREATE_TIMEOUT;
use crate::error::CloudError;
use crate::names::{LABEL_AGENT, LABEL_INSTALL, LABEL_OWNER};
use crate::provider::{MachineOrigin, MachineStatus};

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

/// What a node speaking `protocol` answers `claim` with.
fn claimed(claim: &NodeClaim, protocol: u32) -> crate::ExecOutput {
    let answer = NodeClaimed {
        node_id: claim.node_id.clone(),
        port: 8080,
        sealed: claim.seal.len(),
        protocol,
        tools: vec!["git_ship".to_string()],
    };
    crate::ExecOutput {
        exit_code: 0,
        stdout: serde_json::to_vec(&answer).expect("json"),
        stderr: Vec::new(),
    }
}

/// What a node from before [`NODE_PROTOCOL`] answers a claim with: the
/// same, without the protocol.
fn claimed_by_an_older_node(stdin: Option<&[u8]>) -> crate::ExecOutput {
    let claim: NodeClaim = serde_json::from_slice(stdin.unwrap_or_default()).expect("claim json");
    let answer = serde_json::json!({
        "node_id": claim.node_id,
        "port": 8080,
        "sealed": claim.seal.len(),
    });
    crate::ExecOutput {
        exit_code: 0,
        stdout: serde_json::to_vec(&answer).expect("json"),
        stderr: Vec::new(),
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
        claimed(&claim, NODE_PROTOCOL)
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
mod updates;
