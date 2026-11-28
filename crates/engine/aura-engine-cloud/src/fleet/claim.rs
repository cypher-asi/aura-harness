//! Claiming a machine's node for its agent, and taking back the machines
//! placed agents had.

use crate::boot::{BootReporter, BootStage};
use crate::error::{CloudError, CloudResult};
use crate::names::{self, LABEL_OWNER};
use crate::provider::{MachineInfo, MachineStatus};
use aura_config::BoxdToken;
use aura_core_protocol::cloud::{GitClaim, NodeClaim, NodeClaimed, PlanTemplate, NODE_PROTOCOL};
use aura_core_types::time::now_ms;
use aura_core_types::{VmEvent, VmPayload};
use std::time::Instant;

use super::{
    random_token, Entry, MachineView, VmFleet, VmState, AUTO_SUSPEND_SECS, CLAIM_COMMAND,
    CLAIM_TIMEOUT,
};

/// Which nodes a claim takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Accept {
    /// Any node. One older than this hub is still claimed, so its agent
    /// can be brought back; new work checks it first
    /// ([`VmFleet::check_node_protocol`]).
    Any,
    /// Only a node that speaks this hub's [`NODE_PROTOCOL`]: an older one
    /// fails the claim.
    Current,
}

/// [`CloudError::OutdatedNode`] unless `protocol`, what `machine`'s node
/// answered its claim with, is at least this hub's [`NODE_PROTOCOL`].
fn check_protocol(machine: &str, protocol: u32) -> CloudResult<()> {
    if protocol >= NODE_PROTOCOL {
        return Ok(());
    }
    Err(CloudError::OutdatedNode {
        machine: machine.to_string(),
        has: protocol,
        needs: NODE_PROTOCOL,
    })
}

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
            public_url: Some(target.machine.url()),
        };
        let claimed = self.send_claim(&target.machine, &claim).await?;
        self.mark_claimed(agent, node_token, claimed.protocol);
        self.ensure_run_alone(agent, &target.machine.name).await;
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
    /// answered. A node `accept` doesn't take fails the stage.
    pub(super) async fn claim_reported(
        &self,
        agent: &str,
        machine: &str,
        seal: &[String],
        accept: Accept,
        boot: &BootReporter,
    ) -> CloudResult<NodeClaimed> {
        let claimed = boot
            .run(BootStage::Claim, async {
                let claimed = boot
                    .wait(
                        BootStage::Claim,
                        claim_waits_on(machine),
                        Some(CLAIM_TIMEOUT),
                        self.claim(agent, seal),
                    )
                    .await?;
                if accept == Accept::Current {
                    check_protocol(machine, claimed.protocol)?;
                }
                Ok::<_, CloudError>(claimed)
            })
            .await?;
        claim_facts(boot, &claimed);
        Ok(claimed)
    }

    /// [`CloudError::OutdatedNode`] when `agent`'s node, as last claimed,
    /// speaks an older protocol than this hub, so a turn there would
    /// start without the agent's conversation.
    ///
    /// # Errors
    ///
    /// That, or [`CloudError::NotFound`] when the agent has no machine.
    pub fn check_node_protocol(&self, agent: &str) -> CloudResult<()> {
        let (machine, protocol) = {
            let machines = self.machines.lock();
            let entry = machines.get(agent).ok_or_else(|| CloudError::NotFound {
                what: "machine",
                name: agent.to_string(),
            })?;
            (entry.machine.name.clone(), entry.node_protocol)
        };
        check_protocol(&machine, protocol)
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
    /// is taken to have said (no [`NODE_PROTOCOL`] at all).
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
                protocol: 0,
            }),
        )
    }

    /// Give `machine` the idle settings it keeps working by while no hub
    /// talks to it: no auto-suspend ([`AUTO_SUSPEND_SECS`]), and the
    /// connections it opens itself (model calls, git) keep it awake.
    ///
    /// # Errors
    ///
    /// When boxd refuses either setting.
    pub(super) async fn run_alone(&self, machine: &str) -> CloudResult<()> {
        self.provider
            .set_auto_suspend(machine, AUTO_SUSPEND_SECS)
            .await?;
        self.provider.set_idle_ignore_outbound(machine, false).await
    }

    /// `agent`'s machine has the settings of [`Self::run_alone`].
    pub(super) fn note_run_alone(&self, agent: &str) {
        if let Some(entry) = self.machines.lock().get_mut(agent) {
            entry.idle_set = true;
        }
    }

    /// [`Self::run_alone`] for `agent`'s machine, unless it has the
    /// settings already (a machine made before them, taken back after a
    /// restart). A failure is only logged: its next claim tries again.
    async fn ensure_run_alone(&self, agent: &str, machine: &str) {
        let set = self
            .machines
            .lock()
            .get(agent)
            .is_none_or(|entry| entry.idle_set);
        if set {
            return;
        }
        match self.run_alone(machine).await {
            Ok(()) => self.note_run_alone(agent),
            Err(err) => {
                tracing::warn!(machine, error = %err, "could not set the machine to keep working while the hub is away");
            }
        }
    }

    /// `agent`'s node holds `node_token` now and speaks `protocol`: its
    /// machine is up and ready for work.
    fn mark_claimed(&self, agent: &str, node_token: BoxdToken, protocol: u32) {
        let mut machines = self.machines.lock();
        if let Some(entry) = machines.get_mut(agent) {
            entry.node_token = Some(node_token);
            entry.node_protocol = protocol;
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
                node_protocol: 0,
                usage: None,
                seen_ms: now_ms(),
                idle_set: false,
            },
        );
        self.view(agent).ok_or(CloudError::NotFound {
            what: "machine",
            name: machine.to_string(),
        })
    }
}
