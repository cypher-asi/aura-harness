//! Waking machines, the work that keeps them awake, and idle sleep.

use crate::boot::{BootReporter, BootStage};
use crate::boxd::CALL_TIMEOUT;
use crate::error::{CloudError, CloudResult};
use crate::provider::{MachineInfo, MachineStatus};
use aura_core_types::time::now_ms;
use aura_core_types::{VmEvent, VmPayload};
use std::sync::Arc;
use std::time::Instant;

use super::{Entry, MachineView, VmFleet, VmState};

/// What waking needs from an agent's machine entry.
struct WakeTarget {
    machine: MachineInfo,
    state: VmState,
    ledger: String,
    /// Whether its node holds a token already.
    claimed: bool,
}

/// How an idle machine goes to sleep: the state, the ledger event and
/// the machine status it ends up with.
struct Sleep {
    state: VmState,
    event: VmEvent,
    status: MachineStatus,
}

impl Sleep {
    fn new(hibernate: bool) -> Self {
        if hibernate {
            Self {
                state: VmState::Hibernated,
                event: VmEvent::Hibernated,
                status: MachineStatus::Hibernated,
            }
        } else {
            Self {
                state: VmState::Standby,
                event: VmEvent::Standby,
                status: MachineStatus::Standby,
            }
        }
    }
}

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
        let target = self.wake_target(agent)?;
        if target.state == VmState::Destroying {
            return Err(CloudError::BadState {
                machine: target.machine.name,
                state: target.state.label().to_string(),
                op: "wake",
            });
        }
        if target.state.is_asleep() || matches!(target.state, VmState::Error(_)) {
            self.wake_reported(agent, &target, boot).await?;
        }
        if !target.claimed {
            self.claim_reported(agent, &target.machine.name, &[], boot)
                .await?;
        }
        self.view(agent).ok_or(CloudError::NotFound {
            what: "machine",
            name: target.machine.name,
        })
    }

    fn wake_target(&self, agent: &str) -> CloudResult<WakeTarget> {
        let machines = self.machines.lock();
        let entry = machines.get(agent).ok_or_else(|| CloudError::NotFound {
            what: "machine",
            name: agent.to_string(),
        })?;
        Ok(WakeTarget {
            machine: entry.machine.clone(),
            state: entry.state.clone(),
            ledger: entry.ledger.clone(),
            claimed: entry.node_token.is_some(),
        })
    }

    /// Wake `agent`'s sleeping machine as the [`BootStage::Wake`] stage,
    /// recording how long it took.
    async fn wake_reported(
        &self,
        agent: &str,
        target: &WakeTarget,
        boot: &BootReporter,
    ) -> CloudResult<()> {
        let WakeTarget {
            machine,
            state,
            ledger,
            ..
        } = target;
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
            ledger,
            &VmPayload {
                boot_ms: Some(boot_ms),
                ..Self::base_row(VmEvent::Woke, &info)
            },
        );
        let mut machines = self.machines.lock();
        if let Some(entry) = machines.get_mut(agent) {
            entry.machine.status = MachineStatus::Running;
            entry.state = VmState::Ready;
            entry.idle_since = Instant::now();
        }
        Ok(())
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
        let mut slept = Vec::new();
        for due in self.idle_machines() {
            if self.sleep_idle(&due).await {
                slept.push(due.agent);
            }
        }
        slept
    }

    /// How `entry` should sleep now, if it should: `Some(true)` to
    /// hibernate, `Some(false)` for standby.
    fn sleep_due(&self, entry: &Entry, now: Instant) -> Option<bool> {
        if entry.busy > 0 {
            return None;
        }
        let idle = now.saturating_duration_since(entry.idle_since);
        let hibernate = !self.settings.idle_hibernate.is_zero()
            && matches!(entry.state, VmState::Ready | VmState::Standby)
            && idle >= self.settings.idle_hibernate;
        let standby = !self.settings.idle_standby.is_zero()
            && entry.state == VmState::Ready
            && idle >= self.settings.idle_standby
            && !hibernate;
        (hibernate || standby).then_some(hibernate)
    }

    /// The machines idle long enough to sleep now. Nothing changes until
    /// [`Self::sleep_idle`], so the caller can save what they hold first.
    #[must_use]
    pub fn idle_machines(&self) -> Vec<IdleMachine> {
        let now = Instant::now();
        let machines = self.machines.lock();
        let mut due: Vec<IdleMachine> = machines
            .iter()
            .filter_map(|(agent, entry)| {
                self.sleep_due(entry, now).map(|hibernate| IdleMachine {
                    agent: agent.clone(),
                    hibernate,
                    awake: entry.state == VmState::Ready,
                })
            })
            .collect();
        due.sort_by(|a, b| a.agent.cmp(&b.agent));
        due
    }

    /// Put `due` to sleep, unless it is no longer idle (work came in
    /// since [`Self::idle_machines`] listed it). Returns whether it went
    /// to sleep.
    pub async fn sleep_idle(&self, due: &IdleMachine) -> bool {
        let Some((machine, ledger)) = self.still_idle(due) else {
            return false;
        };
        let result = if due.hibernate {
            self.provider.hibernate(&machine.name).await
        } else {
            self.provider.standby(&machine.name).await
        };
        match result {
            Ok(()) => {
                self.mark_asleep(&due.agent, &machine, &ledger, Sleep::new(due.hibernate));
                true
            }
            Err(err) => {
                tracing::warn!(machine = %machine.name, error = %err, "could not put an idle machine to sleep");
                false
            }
        }
    }

    /// `due`'s machine and ledger, if it is still due to sleep the way
    /// it was listed.
    fn still_idle(&self, due: &IdleMachine) -> Option<(MachineInfo, String)> {
        let machines = self.machines.lock();
        let entry = machines.get(&due.agent)?;
        if self.sleep_due(entry, Instant::now()) != Some(due.hibernate) {
            return None;
        }
        Some((entry.machine.clone(), entry.ledger.clone()))
    }

    /// Record that `agent`'s `machine` went to sleep as `sleep` says.
    fn mark_asleep(&self, agent: &str, machine: &MachineInfo, ledger: &str, sleep: Sleep) {
        let mut info = machine.clone();
        info.status = sleep.status.clone();
        self.row(
            ledger,
            &VmPayload {
                reason: Some("idle".into()),
                ..Self::base_row(sleep.event, &info)
            },
        );
        if let Some(entry) = self.machines.lock().get_mut(agent) {
            // Work may have started while the call ran.
            if entry.busy == 0 {
                entry.state = sleep.state;
                entry.machine.status = sleep.status;
            }
        }
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
                    if self.note_seen(&agent, &state, info) {
                        woke.push(agent.clone());
                    }
                }
                Ok(None) if state != VmState::Destroying => self.note_gone(&agent, &ledger),
                Ok(None) => {}
                Err(err) => tracing::debug!(machine = %name, error = %err, "machine poll failed"),
            }
        }
        woke
    }

    /// Take in boxd's `info` on `agent`'s machine, which the fleet had
    /// in state `was`. Returns whether it woke up without the fleet
    /// waking it.
    fn note_seen(&self, agent: &str, was: &VmState, info: MachineInfo) -> bool {
        let mut machines = self.machines.lock();
        let Some(entry) = machines.get_mut(agent) else {
            return false;
        };
        entry.seen_ms = now_ms();
        let mut woke = false;
        match (was, &info.status) {
            (VmState::Standby | VmState::Hibernated, MachineStatus::Running) => {
                entry.state = VmState::Ready;
                entry.idle_since = Instant::now();
                woke = true;
            }
            (VmState::Ready, MachineStatus::Hibernated) => {
                entry.state = VmState::Hibernated;
            }
            (VmState::Ready, MachineStatus::Standby) => entry.state = VmState::Standby,
            _ => {}
        }
        entry.machine = info;
        woke
    }

    /// Drop `agent`'s machine, gone from boxd, with an `error` row.
    fn note_gone(&self, agent: &str, ledger: &str) {
        if let Some(entry) = self.machines.lock().remove(agent) {
            self.row(
                ledger,
                &VmPayload {
                    reason: Some("machine disappeared".into()),
                    ..Self::base_row(VmEvent::Error, &entry.machine)
                },
            );
        }
    }
}

/// A machine idle long enough to sleep ([`VmFleet::idle_machines`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdleMachine {
    /// Agent, hex.
    pub agent: String,
    /// Hibernate rather than stand by.
    pub hibernate: bool,
    /// Awake now: what it wrote since its last copy is still only on it.
    pub awake: bool,
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
