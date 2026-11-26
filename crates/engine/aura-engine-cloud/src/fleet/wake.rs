//! Waking machines, the work that keeps them awake, and idle sleep.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use aura_config::{BoxdToken, CloudConfig, MachineSize};
use aura_core_protocol::cloud::{GitClaim, NodeClaim, NodeClaimed, PlanTemplate};
use aura_core_types::time::now_ms;
use aura_core_types::{VmEvent, VmPayload, VmResources, VmSource};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use crate::boot::{human_bytes, BootReporter, BootStage};
use crate::boxd::{CALL_TIMEOUT, CREATE_TIMEOUT};
use crate::error::{CloudError, CloudResult};
use crate::machine_env::MachineEnv;
use crate::names::{
    self, LABEL_AGENT, LABEL_INSTALL, LABEL_OWNER, LABEL_PROJECT, LABEL_ROLE, ROLE_AGENT,
};
use crate::provider::{
    MachineInfo, MachineOrigin, MachineStatus, NewMachine, UsageReading, VmProvider,
};

use super::{AUTO_HIBERNATE_BACKSTOP_SECS, CLAIM_COMMAND, CLAIM_TIMEOUT, Entry, FleetSettings, MachineView, MemoryLedger, ProvisionRequest, VmFleet, VmLedger, VmState, mesh_key, random_token};

impl VmFleet {
    /// Wake `agent`'s machine if it sleeps, and claim it if it has no
    /// token yet, so it can take work.
    ///
    /// # Errors
    ///
    /// When it has no machine, or waking or claiming fails.
    pub async fn ensure_awake(&self, agent: &str) -> CloudResult<MachineView> {
        self.ensure_awake_reported(agent, &BootReporter::none())
            .await
    }

    /// [`Self::ensure_awake`], showing a wake and a claim on `boot`; a
    /// machine that is already up shows nothing.
    ///
    /// # Errors
    ///
    /// As [`Self::ensure_awake`].
    pub async fn ensure_awake_reported(
        &self,
        agent: &str,
        boot: &BootReporter,
    ) -> CloudResult<MachineView> {
        let (machine, state, ledger, claimed) = {
            let machines = self.machines.lock();
            let entry = machines.get(agent).ok_or_else(|| CloudError::NotFound {
                what: "machine",
                name: agent.to_string(),
            })?;
            (
                entry.machine.clone(),
                entry.state.clone(),
                entry.ledger.clone(),
                entry.node_token.is_some(),
            )
        };
        if state == VmState::Destroying {
            return Err(CloudError::BadState {
                machine: machine.name,
                state: state.label().to_string(),
                op: "wake",
            });
        }
        if state.is_asleep() || matches!(state, VmState::Error(_)) {
            boot.fact("machine", machine.name.clone());
            boot.fact("woke from", state.label().to_string());
            self.set_state(agent, VmState::Waking);
            let started = Instant::now();
            boot.run(
                BootStage::Wake,
                boot.wait(
                    BootStage::Wake,
                    format!("boxd is waking {} from {}", machine.name, state.label()),
                    Some(CALL_TIMEOUT),
                    self.provider.wake(&machine.name),
                ),
            )
            .await?;
            let boot_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            boot.fact("wake time", format!("{boot_ms} ms"));
            let mut info = machine.clone();
            info.status = MachineStatus::Running;
            self.row(
                &ledger,
                &VmPayload {
                    boot_ms: Some(boot_ms),
                    ..Self::base_row(VmEvent::Woke, &info)
                },
            );
            {
                let mut machines = self.machines.lock();
                if let Some(entry) = machines.get_mut(agent) {
                    entry.machine.status = MachineStatus::Running;
                    entry.state = VmState::Ready;
                    entry.idle_since = Instant::now();
                }
            }
        }
        if !claimed {
            let claimed = boot
                .run(
                    BootStage::Claim,
                    boot.wait(
                        BootStage::Claim,
                        claim_waits_on(&machine.name),
                        Some(CLAIM_TIMEOUT),
                        self.claim(agent, &[]),
                    ),
                )
                .await?;
            claim_facts(boot, &claimed);
        }
        self.view(agent).ok_or(CloudError::NotFound {
            what: "machine",
            name: machine.name,
        })
    }

    pub(super) fn set_state(&self, agent: &str, state: VmState) {
        if let Some(entry) = self.machines.lock().get_mut(agent) {
            entry.state = state;
        }
    }

    /// Mark work in flight on `agent`'s machine; it can't sleep until
    /// the returned guard drops.
    #[must_use]
    pub fn begin_work(self: &Arc<Self>, agent: &str) -> WorkGuard {
        if let Some(entry) = self.machines.lock().get_mut(agent) {
            entry.busy += 1;
            entry.state = VmState::Busy;
        }
        WorkGuard {
            fleet: Arc::clone(self),
            agent: agent.to_string(),
        }
    }

    pub(super) fn end_work(&self, agent: &str) {
        if let Some(entry) = self.machines.lock().get_mut(agent) {
            entry.busy = entry.busy.saturating_sub(1);
            if entry.busy == 0 {
                entry.idle_since = Instant::now();
                if entry.state == VmState::Busy {
                    entry.state = VmState::Ready;
                }
            }
        }
    }

    /// Reset `agent`'s idle clock (mail arrived, the user looked at it).
    pub fn touch(&self, agent: &str) {
        if let Some(entry) = self.machines.lock().get_mut(agent) {
            entry.idle_since = Instant::now();
        }
    }

    /// Put idle machines to sleep. Returns the agents whose machines
    /// went to sleep. Call it every few seconds.
    pub async fn tick(&self) -> Vec<String> {
        let now = Instant::now();
        let mut due: Vec<(String, MachineInfo, String, bool)> = Vec::new();
        {
            let machines = self.machines.lock();
            for (agent, entry) in machines.iter() {
                if entry.busy > 0 {
                    continue;
                }
                let idle = now.saturating_duration_since(entry.idle_since);
                let standby = !self.settings.idle_standby.is_zero()
                    && entry.state == VmState::Ready
                    && idle >= self.settings.idle_standby
                    && (self.settings.idle_hibernate.is_zero()
                        || idle < self.settings.idle_hibernate);
                let hibernate = !self.settings.idle_hibernate.is_zero()
                    && matches!(entry.state, VmState::Ready | VmState::Standby)
                    && idle >= self.settings.idle_hibernate;
                if hibernate || standby {
                    due.push((
                        agent.clone(),
                        entry.machine.clone(),
                        entry.ledger.clone(),
                        hibernate,
                    ));
                }
            }
        }
        let mut slept = Vec::new();
        for (agent, machine, ledger, hibernate) in due {
            let (result, state, event, status) = if hibernate {
                (
                    self.provider.hibernate(&machine.name).await,
                    VmState::Hibernated,
                    VmEvent::Hibernated,
                    MachineStatus::Hibernated,
                )
            } else {
                (
                    self.provider.standby(&machine.name).await,
                    VmState::Standby,
                    VmEvent::Standby,
                    MachineStatus::Standby,
                )
            };
            match result {
                Ok(()) => {
                    let mut info = machine.clone();
                    info.status = status.clone();
                    self.row(
                        &ledger,
                        &VmPayload {
                            reason: Some("idle".into()),
                            ..Self::base_row(event, &info)
                        },
                    );
                    if let Some(entry) = self.machines.lock().get_mut(&agent) {
                        // Work may have started while the call ran.
                        if entry.busy == 0 {
                            entry.state = state;
                            entry.machine.status = status;
                        }
                    }
                    slept.push(agent);
                }
                Err(err) => {
                    tracing::warn!(machine = %machine.name, error = %err, "could not put an idle machine to sleep");
                }
            }
        }
        slept
    }

    /// Re-read every machine from boxd: notice machines woken by a peer
    /// or by boxd's timers, and drop machines that are gone. Returns the
    /// agents whose machine woke up without the fleet waking it (their
    /// sockets must be reopened).
    pub async fn poll(&self) -> Vec<String> {
        let snapshot: Vec<(String, String, VmState, String)> = self
            .machines
            .lock()
            .iter()
            .map(|(agent, e)| {
                (
                    agent.clone(),
                    e.machine.name.clone(),
                    e.state.clone(),
                    e.ledger.clone(),
                )
            })
            .collect();
        let mut woke = Vec::new();
        for (agent, name, state, ledger) in snapshot {
            match self.provider.get(&name).await {
                Ok(Some(info)) => {
                    let mut machines = self.machines.lock();
                    let Some(entry) = machines.get_mut(&agent) else {
                        continue;
                    };
                    entry.seen_ms = now_ms();
                    match (&state, &info.status) {
                        (VmState::Standby | VmState::Hibernated, MachineStatus::Running) => {
                            entry.state = VmState::Ready;
                            entry.idle_since = Instant::now();
                            woke.push(agent.clone());
                        }
                        (VmState::Ready, MachineStatus::Hibernated) => {
                            entry.state = VmState::Hibernated;
                        }
                        (VmState::Ready, MachineStatus::Standby) => entry.state = VmState::Standby,
                        _ => {}
                    }
                    entry.machine = info;
                }
                Ok(None) if state != VmState::Destroying => {
                    if let Some(entry) = self.machines.lock().remove(&agent) {
                        self.row(
                            &ledger,
                            &VmPayload {
                                reason: Some("machine disappeared".into()),
                                ..Self::base_row(VmEvent::Error, &entry.machine)
                            },
                        );
                    }
                }
                Ok(None) => {}
                Err(err) => tracing::debug!(machine = %name, error = %err, "machine poll failed"),
            }
        }
        woke
    }
}

/// Keeps a machine awake while work runs on it.
pub struct WorkGuard {
    fleet: Arc<VmFleet>,
    agent: String,
}

impl Drop for WorkGuard {
    fn drop(&mut self) {
        self.fleet.end_work(&self.agent);
    }
}
