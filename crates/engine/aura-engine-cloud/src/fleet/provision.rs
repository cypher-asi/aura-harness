//! Making an agent's machine: create, configure and claim it.

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

/// What boxd says about a machine it just made, as boot facts.
pub(super) fn machine_facts(boot: &BootReporter, machine: &MachineInfo, networks: &[String]) {
    boot.fact("machine", machine.name.clone());
    boot.fact(
        "region",
        if machine.region.is_empty() {
            "home region".to_string()
        } else {
            machine.region.clone()
        },
    );
    boot.fact(
        "size",
        format!(
            "{} vCPU · {} memory · {} disk",
            machine.vcpu,
            human_bytes(machine.memory_bytes),
            human_bytes(machine.disk_bytes)
        ),
    );
    if let Some(source) = &machine.source {
        boot.fact(
            "source",
            match source {
                VmSource::Snapshot { name, version } => match version {
                    Some(version) => format!("snapshot {name} v{version}"),
                    None => format!("snapshot {name}"),
                },
                VmSource::Fork { parent } => format!("fork of {parent}"),
                VmSource::Image { image } if image.is_empty() => "boxd default image".into(),
                VmSource::Image { image } => format!("image {image}"),
            },
        );
    }
    boot.fact("boot time", format!("{} ms", machine.boot_ms));
    boot.fact(
        "network",
        if machine.isolated {
            "isolated".to_string()
        } else if networks.is_empty() {
            "default".to_string()
        } else {
            networks.join(", ")
        },
    );
    if !machine.domain.is_empty() {
        boot.fact("domain", format!("https://{}", machine.domain));
    }
    if !machine.public_ip.is_empty() {
        boot.fact("address", machine.public_ip.clone());
    }
}

impl VmFleet {
    /// Create, configure and claim a machine for an agent.
    ///
    /// # Errors
    ///
    /// [`CloudError::MachineLimit`] when the org is full; provider and
    /// claim errors otherwise (the machine is destroyed again then).
    pub async fn provision(&self, request: ProvisionRequest) -> CloudResult<MachineView> {
        self.provision_reported(request, &BootReporter::none())
            .await
    }

    /// [`Self::provision`], showing each stage and what boxd reports
    /// about the machine on `boot`.
    ///
    /// # Errors
    ///
    /// As [`Self::provision`].
    pub async fn provision_reported(
        &self,
        request: ProvisionRequest,
        boot: &BootReporter,
    ) -> CloudResult<MachineView> {
        let limit = boot
            .run(BootStage::Quota, async {
                let limit = boot
                    .wait(
                        BootStage::Quota,
                        "boxd is counting the org's machines",
                        Some(CALL_TIMEOUT),
                        self.provider.machine_limit(),
                    )
                    .await?;
                if limit.has_room() {
                    Ok(limit)
                } else {
                    Err(CloudError::MachineLimit {
                        count: limit.count,
                        max: limit.max,
                    })
                }
            })
            .await?;
        boot.fact(
            "quota",
            format!("{} of {} machines in use", limit.count, limit.max),
        );
        if let MachineOrigin::Fork(parent) = &request.origin {
            // A fork inherits its parent's secrets, so never across owners.
            let parent_owner = self
                .machines
                .lock()
                .values()
                .find(|entry| entry.machine.name == *parent || entry.machine.id == *parent)
                .map(|entry| entry.owner.clone());
            if parent_owner.as_deref() != Some(request.owner.as_str()) {
                return Err(CloudError::BadState {
                    machine: parent.clone(),
                    state: "another user's machine".into(),
                    op: "fork it",
                });
            }
        }
        if let Some(foreign) = request
            .secrets
            .iter()
            .find(|name| !names::is_plan_secret_of(name, &request.owner))
        {
            return Err(CloudError::BadState {
                machine: request.name.clone(),
                state: format!("asked for secret {foreign}"),
                op: "list a secret that is not its owner's plan",
            });
        }
        if let Some(foreign) = request
            .repo_secret
            .as_deref()
            .filter(|name| !names::is_repo_secret(name))
        {
            return Err(CloudError::BadState {
                machine: request.name.clone(),
                state: format!("asked for secret {foreign}"),
                op: "list a repository secret that is not one",
            });
        }
        let ledger = names::machine_ledger(&request.agent_zns);
        let mut labels = BTreeMap::new();
        labels.insert(LABEL_OWNER.to_string(), request.owner.clone());
        labels.insert(LABEL_INSTALL.to_string(), self.settings.install.clone());
        labels.insert(LABEL_AGENT.to_string(), request.agent.clone());
        labels.insert(LABEL_PROJECT.to_string(), request.project.clone());
        labels.insert(LABEL_ROLE.to_string(), ROLE_AGENT.to_string());
        let spec = NewMachine {
            name: request.name.clone(),
            size: self.settings.size,
            isolated: self.settings.isolated,
            labels,
            env: MachineEnv::for_agent(self.settings.size.vcpu()).into_map(),
            packages: Vec::new(),
            networks: if self.settings.isolated {
                Vec::new()
            } else {
                vec![format!("aura-{}", names::dns_fragment(&request.project))]
            },
            delete_after_secs: None,
            secrets: request
                .secrets
                .iter()
                .cloned()
                .chain(request.repo_secret.clone())
                .collect(),
        };
        let creating = match &request.origin {
            MachineOrigin::Snapshot(snapshot) => {
                format!("boxd is creating {} from snapshot {snapshot}", spec.name)
            }
            MachineOrigin::Fork(parent) => format!("boxd is forking {parent} as {}", spec.name),
            MachineOrigin::Fresh => format!("boxd is creating {}", spec.name),
        };
        let machine = boot
            .run(
                BootStage::Create,
                boot.wait(
                    BootStage::Create,
                    creating,
                    Some(CREATE_TIMEOUT),
                    self.provider.create(&spec, &request.origin),
                ),
            )
            .await?;
        machine_facts(boot, &machine, &spec.networks);
        let event = match request.origin {
            MachineOrigin::Fork(_) => VmEvent::Forked,
            _ => VmEvent::Created,
        };
        let source = match &request.origin {
            MachineOrigin::Snapshot(name) => Some(VmSource::Snapshot {
                name: name.clone(),
                version: None,
            }),
            MachineOrigin::Fork(parent) => Some(VmSource::Fork {
                parent: parent.clone(),
            }),
            MachineOrigin::Fresh => machine.source.clone(),
        };
        self.row(
            &ledger,
            &VmPayload {
                source: machine.source.clone().or(source),
                boot_ms: Some(machine.boot_ms),
                ..Self::base_row(event, &machine)
            },
        );
        self.machines.lock().insert(
            request.agent.clone(),
            Entry {
                owner: request.owner.clone(),
                plans: request.plans.clone(),
                git: request.git.clone(),
                project: request.project.clone(),
                ledger: ledger.clone(),
                machine: machine.clone(),
                state: VmState::Claiming,
                busy: 0,
                idle_since: Instant::now(),
                node_token: None,
                usage: None,
                seen_ms: now_ms(),
            },
        );
        boot.run(BootStage::Configure, async {
            // boxd's bot check would answer API calls with a 503.
            let bot = boot
                .wait(
                    BootStage::Configure,
                    "boxd is turning bot protection off",
                    Some(CALL_TIMEOUT),
                    self.provider.set_bot_protection(&machine.name, false),
                )
                .await;
            if let Err(err) = bot {
                tracing::warn!(machine = %machine.name, error = %err, "could not turn bot protection off");
                boot.log(BootStage::Configure, true, format!("bot protection stays on: {err}"));
            } else {
                boot.log(BootStage::Configure, false, "bot protection off for the node API");
            }
            match boot
                .wait(
                    BootStage::Configure,
                    "boxd is setting the hibernation backstop",
                    Some(CALL_TIMEOUT),
                    self.provider
                        .set_auto_hibernate(&machine.name, AUTO_HIBERNATE_BACKSTOP_SECS),
                )
                .await
            {
                Ok(()) => boot.log(
                    BootStage::Configure,
                    false,
                    format!(
                        "boxd hibernates it after {} min idle",
                        AUTO_HIBERNATE_BACKSTOP_SECS / 60
                    ),
                ),
                Err(err) => {
                    tracing::warn!(machine = %machine.name, error = %err, "could not set the hibernation backstop");
                    boot.log(
                        BootStage::Configure,
                        true,
                        format!("no hibernation backstop: {err}"),
                    );
                }
            }
            Ok::<_, CloudError>(())
        })
        .await?;
        let claimed = boot
            .run(
                BootStage::Claim,
                boot.wait(
                    BootStage::Claim,
                    claim_waits_on(&machine.name),
                    Some(CLAIM_TIMEOUT),
                    self.claim(&request.agent, &request.seal),
                ),
            )
            .await;
        if let Ok(claimed) = &claimed {
            claim_facts(boot, claimed);
        }
        if let Err(err) = claimed {
            self.row(
                &ledger,
                &VmPayload {
                    reason: Some(err.to_string()),
                    ..Self::base_row(VmEvent::Error, &machine)
                },
            );
            let _ = self.provider.destroy(&machine.name).await;
            self.machines.lock().remove(&request.agent);
            return Err(err);
        }
        self.view(&request.agent).ok_or(CloudError::NotFound {
            what: "machine",
            name: machine.name,
        })
    }
}
