//! Claiming a machine's node for its agent, and taking back the machines
//! placed agents had.

use crate::boot::BootReporter;
use crate::error::{CloudError, CloudResult};
use crate::names::{self, LABEL_OWNER};
use crate::provider::MachineStatus;
use aura_core_protocol::cloud::{GitClaim, NodeClaim, NodeClaimed, PlanTemplate};
use aura_core_types::time::now_ms;
use aura_core_types::{VmEvent, VmPayload};
use std::time::Instant;

use super::{random_token, Entry, MachineView, VmFleet, VmState, CLAIM_COMMAND, CLAIM_TIMEOUT};

/// What a node answered its claim with, as boot facts.
pub(super) fn claim_facts(boot: &BootReporter, claimed: &NodeClaimed) {
    boot.fact(
        "node",
        format!("{} on port {}", claimed.node_id, claimed.port),
    );
    if claimed.sealed > 0 {
        boot.fact("sealed", format!("{} inherited agents", claimed.sealed));
    }
}

/// What a claim waits on, as shown: the claim's own output carries
/// tokens and is never streamed, so this is all a boot shows of it.
pub(super) fn claim_waits_on(machine: &str) -> String {
    format!("`{CLAIM_COMMAND}` on {machine} (the node starts and takes its tokens)")
}

impl VmFleet {
    /// Give `agent`'s node fresh tokens over `Exec`, sealing `seal`.
    ///
    /// # Errors
    ///
    /// When the command fails or answers something unreadable.
    pub async fn claim(&self, agent: &str, seal: &[String]) -> CloudResult<NodeClaimed> {
        let (machine, project, ledger, owner, plans, git) = {
            let machines = self.machines.lock();
            let entry = machines.get(agent).ok_or_else(|| CloudError::NotFound {
                what: "machine",
                name: agent.to_string(),
            })?;
            (
                entry.machine.clone(),
                entry.project.clone(),
                entry.ledger.clone(),
                entry.owner.clone(),
                entry.plans.clone(),
                entry.git.clone(),
            )
        };
        let node_token = random_token();
        let claim = NodeClaim {
            node_id: machine.name.clone(),
            node_token: node_token.expose().to_string(),
            mesh_token: self.mesh_token(&owner, &project).expose().to_string(),
            seal: seal.to_vec(),
            owner: (!owner.is_empty()).then(|| owner.clone()),
            plans,
            git,
        };
        let stdin = serde_json::to_vec(&claim).map_err(|err| CloudError::InvalidResponse {
            op: "claim",
            message: err.to_string(),
        })?;
        let output = self
            .provider
            .exec(&machine.name, CLAIM_COMMAND, Some(stdin), CLAIM_TIMEOUT)
            .await?;
        if !output.success() {
            return Err(CloudError::ExecFailed {
                machine: machine.name.clone(),
                command: CLAIM_COMMAND.to_string(),
                exit_code: output.exit_code,
                stderr: output.stderr_tail(400),
            });
        }
        let claimed: NodeClaimed = serde_json::from_slice(&output.stdout).unwrap_or(NodeClaimed {
            node_id: machine.name.clone(),
            port: crate::machine_env::NODE_PORT,
            sealed: seal.len(),
        });
        {
            let mut machines = self.machines.lock();
            if let Some(entry) = machines.get_mut(agent) {
                entry.node_token = Some(node_token);
                entry.state = VmState::Ready;
                entry.machine.status = MachineStatus::Running;
                entry.idle_since = Instant::now();
            }
        }
        self.row(
            &ledger,
            &VmPayload {
                reason: (!seal.is_empty()).then(|| format!("sealed {} agents", claimed.sealed)),
                ..Self::base_row(VmEvent::Claimed, &machine)
            },
        );
        Ok(claimed)
    }

    /// Take over a machine that already exists for `agent` (after a hub
    /// restart). It is claimed again with fresh tokens when it next
    /// runs ([`Self::ensure_awake`]).
    ///
    /// # Errors
    ///
    /// [`CloudError::NotFound`] when the machine is gone.
    pub async fn adopt(
        &self,
        agent: &str,
        agent_zns: &str,
        project: &str,
        machine: &str,
        plans: Vec<PlanTemplate>,
        git: Option<GitClaim>,
    ) -> CloudResult<MachineView> {
        let info = self
            .provider
            .get(machine)
            .await?
            .ok_or_else(|| CloudError::NotFound {
                what: "machine",
                name: machine.to_string(),
            })?;
        let state = match info.status {
            MachineStatus::Standby => VmState::Standby,
            MachineStatus::Hibernated | MachineStatus::Stopped => VmState::Hibernated,
            MachineStatus::Error => VmState::Error("boxd reports an error".into()),
            _ => VmState::Claiming,
        };
        let owner = info.labels.get(LABEL_OWNER).cloned().unwrap_or_default();
        self.machines.lock().insert(
            agent.to_string(),
            Entry {
                owner,
                plans,
                git,
                project: project.to_string(),
                ledger: names::machine_ledger(agent_zns),
                machine: info,
                state,
                busy: 0,
                idle_since: Instant::now(),
                node_token: None,
                usage: None,
                seen_ms: now_ms(),
            },
        );
        self.view(agent).ok_or(CloudError::NotFound {
            what: "machine",
            name: machine.to_string(),
        })
    }
}
