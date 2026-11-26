//! Metered usage of the machines.

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
    /// Read every machine's meter and write a `usage` row for each.
    pub async fn record_usage(&self) {
        if *self.usage_denied.lock() {
            return;
        }
        let targets: Vec<(String, MachineInfo, String)> = self
            .machines
            .lock()
            .iter()
            .map(|(agent, e)| (agent.clone(), e.machine.clone(), e.ledger.clone()))
            .collect();
        for (agent, machine, ledger) in targets {
            match self.provider.machine_usage(&machine.id).await {
                Ok(reading) => {
                    self.row(
                        &ledger,
                        &VmPayload {
                            usage: Some(reading.usage),
                            currency: Some(reading.currency.clone()),
                            ..Self::base_row(VmEvent::Usage, &machine)
                        },
                    );
                    if let Some(entry) = self.machines.lock().get_mut(&agent) {
                        entry.usage = Some(reading);
                    }
                }
                Err(err) if err.is_permission_denied() => {
                    tracing::info!("boxd usage needs an org admin key; costs are estimated");
                    *self.usage_denied.lock() = true;
                    return;
                }
                Err(err) => {
                    tracing::debug!(machine = %machine.name, error = %err, "usage read failed");
                }
            }
        }
    }

    /// Whether boxd refused usage calls.
    #[must_use]
    pub fn usage_denied(&self) -> bool {
        *self.usage_denied.lock()
    }

    /// After a destroy, read the machine's meter until boxd marks it
    /// complete (or `attempts` run out, `every` apart) and record the
    /// final figure on `ledger`.
    pub async fn settle_usage(
        &self,
        machine_id: &str,
        name: &str,
        ledger: &str,
        attempts: u32,
        every: Duration,
    ) {
        if self.usage_denied() {
            return;
        }
        for attempt in 0..attempts.max(1) {
            if attempt > 0 {
                tokio::time::sleep(every).await;
            }
            match self.provider.machine_usage(machine_id).await {
                Ok(reading) if reading.usage.complete => {
                    self.row(
                        ledger,
                        &VmPayload {
                            usage: Some(reading.usage),
                            currency: Some(reading.currency),
                            reason: Some("final".into()),
                            ..VmPayload::new(VmEvent::Usage, machine_id, name)
                        },
                    );
                    return;
                }
                Ok(_) => {}
                Err(err) if err.is_permission_denied() => return,
                Err(err) => {
                    tracing::debug!(machine = %name, error = %err, "final usage read failed");
                }
            }
        }
    }
}
