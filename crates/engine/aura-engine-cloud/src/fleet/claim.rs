//! Claiming a machine's node for its agent, and taking back the machines
//! placed agents had.

use crate::boot::{BootReporter, BootStage};
use crate::error::{CloudError, CloudResult};
use crate::names::{self, LABEL_OWNER};
use crate::provider::{MachineInfo, MachineStatus};
use aura_config::BoxdToken;
use aura_core_protocol::cloud::{GitClaim, NodeClaim, NodeClaimed, PlanTemplate};
use aura_core_types::time::now_ms;
use aura_core_types::{VmEvent, VmPayload};
use std::time::Instant;

use super::{random_token, Entry, MachineView, VmFleet, VmState, CLAIM_COMMAND, CLAIM_TIMEOUT};

/// What a node answered its claim with, as boot facts.
fn claim_facts(boot: &BootReporter, claimed: &NodeClaimed) {
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
fn claim_waits_on(machine: &str) -> String {
    format!("`{CLAIM_COMMAND}` on {machine} (the node starts and takes its tokens)")
}

/// What a claim needs from an agent's machine entry.
struct ClaimTarget {
    machine: MachineInfo,
    project: String,
    ledger: String,
    owner: String,
    plans: Vec<PlanTemplate>,
    git: Option<GitClaim>,
}

impl VmFleet {
    /// Give `agent`'s node fresh tokens over `Exec`, sealing `seal`.
    ///
    /// # Errors
    ///
    /// When the command fails or answers something unreadable.
    pub async fn claim(&self, agent: &str, seal: &[String]) -> CloudResult<NodeClaimed> {
        let target = self.claim_target(agent)?;
        let node_token = random_token();
        let claim = NodeClaim {
            node_id: target.machine.name.clone(),
            node_token: node_token.expose().to_string(),
            mesh_token: self
                .mesh_token(&target.owner, &target.project)
                .expose()
                .to_string(),
            seal: seal.to_vec(),
            owner: (!target.owner.is_empty()).then(|| target.owner.clone()),
            plans: target.plans,
            git: target.git,
        };
        let claimed = self.send_claim(&target.machine, &claim).await?;
        self.mark_claimed(agent, node_token);
        self.row(
            &target.ledger,
            &VmPayload {
                reason: (!seal.is_empty()).then(|| format!("sealed {} agents", claimed.sealed)),
                ..Self::base_row(VmEvent::Claimed, &target.machine)
            },
        );
        Ok(claimed)
    }

    /// [`Self::claim`] of `agent`'s machine `machine` as the
    /// [`BootStage::Claim`] stage of `boot`, showing what the node
    /// answered.
    pub(super) async fn claim_reported(
        &self,
        agent: &str,
        machine: &str,
        seal: &[String],
        boot: &BootReporter,
    ) -> CloudResult<NodeClaimed> {
        let claimed = boot
            .run(
                BootStage::Claim,
                boot.wait(
                    BootStage::Claim,
                    claim_waits_on(machine),
                    Some(CLAIM_TIMEOUT),
                    self.claim(agent, seal),
                ),
            )
            .await?;
        claim_facts(boot, &claimed);
        Ok(claimed)
    }

    fn claim_target(&self, agent: &str) -> CloudResult<ClaimTarget> {
        let machines = self.machines.lock();
        let entry = machines.get(agent).ok_or_else(|| CloudError::NotFound {
            what: "machine",
            name: agent.to_string(),
        })?;
        Ok(ClaimTarget {
            machine: entry.machine.clone(),
            project: entry.project.clone(),
            ledger: entry.ledger.clone(),
            owner: entry.owner.clone(),
            plans: entry.plans.clone(),
            git: entry.git.clone(),
        })
    }

    /// Run [`CLAIM_COMMAND`] on `machine` with `claim` on its stdin: what
    /// the node answered, or what a node that answers nothing readable
    /// is taken to have said.
    async fn send_claim(
        &self,
        machine: &MachineInfo,
        claim: &NodeClaim,
    ) -> CloudResult<NodeClaimed> {
        let stdin = serde_json::to_vec(claim).map_err(|err| CloudError::InvalidResponse {
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
        Ok(
            serde_json::from_slice(&output.stdout).unwrap_or(NodeClaimed {
                node_id: machine.name.clone(),
                port: crate::machine_env::NODE_PORT,
                sealed: claim.seal.len(),
            }),
        )
    }

    /// `agent`'s node holds `node_token` now: its machine is up and
    /// ready for work.
    fn mark_claimed(&self, agent: &str, node_token: BoxdToken) {
        let mut machines = self.machines.lock();
        if let Some(entry) = machines.get_mut(agent) {
            entry.node_token = Some(node_token);
            entry.state = VmState::Ready;
            entry.machine.status = MachineStatus::Running;
            entry.idle_since = Instant::now();
        }
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
