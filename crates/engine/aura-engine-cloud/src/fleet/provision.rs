//! Making an agent's machine: create, configure and claim it.

use crate::boot::{human_bytes, BootReporter, BootStage};
use crate::boxd::{CALL_TIMEOUT, CREATE_TIMEOUT};
use crate::error::{CloudError, CloudResult};
use crate::machine_env::MachineEnv;
use crate::names::{
    self, LABEL_AGENT, LABEL_INSTALL, LABEL_OWNER, LABEL_PROJECT, LABEL_ROLE, ROLE_AGENT,
};
use crate::provider::{MachineInfo, MachineOrigin, NewMachine};
use aura_core_types::time::now_ms;
use aura_core_types::{VmEvent, VmPayload, VmSource};
use std::collections::BTreeMap;
use std::time::Instant;

use super::claim::Accept;
use super::{Entry, MachineView, ProvisionRequest, VmFleet, VmState, AUTO_HIBERNATE_BACKSTOP_SECS};

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

/// Refuse a secret `request` lists that isn't its owner's to list.
fn check_secrets(request: &ProvisionRequest) -> CloudResult<()> {
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
    Ok(())
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
        self.check_quota(boot).await?;
        self.check_request(&request)?;
        let ledger = names::machine_ledger(&request.agent_zns);
        let spec = self.machine_spec(&request);
        let machine = self.create_machine(&spec, &request.origin, boot).await?;
        self.created_row(&request.origin, &machine, &ledger);
        self.keep_claiming(&request, &machine, &ledger);
        self.configure(&machine, boot).await?;
        if let Err(err) = self
            .claim_reported(
                &request.agent,
                &machine.name,
                &request.seal,
                Accept::Current,
                boot,
            )
            .await
        {
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

    /// Make sure the org has room for one more machine, as the
    /// [`BootStage::Quota`] stage.
    async fn check_quota(&self, boot: &BootReporter) -> CloudResult<()> {
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
        Ok(())
    }

    /// Refuse a request that crosses owners: a fork of another user's
    /// machine, or a secret that isn't the owner's.
    fn check_request(&self, request: &ProvisionRequest) -> CloudResult<()> {
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
        check_secrets(request)
    }

    /// The machine `request` asks for: labelled with its owner, agent and
    /// project, on the project's network unless isolated, listing the
    /// owner's plan secrets and the project's repository secret.
    fn machine_spec(&self, request: &ProvisionRequest) -> NewMachine {
        let mut labels = BTreeMap::new();
        labels.insert(LABEL_OWNER.to_string(), request.owner.clone());
        labels.insert(LABEL_INSTALL.to_string(), self.settings.install.clone());
        labels.insert(LABEL_AGENT.to_string(), request.agent.clone());
        labels.insert(LABEL_PROJECT.to_string(), request.project.clone());
        labels.insert(LABEL_ROLE.to_string(), ROLE_AGENT.to_string());
        NewMachine {
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
        }
    }

    /// Create `spec`'s machine from `origin` as the [`BootStage::Create`]
    /// stage, showing what boxd reports about it.
    async fn create_machine(
        &self,
        spec: &NewMachine,
        origin: &MachineOrigin,
        boot: &BootReporter,
    ) -> CloudResult<MachineInfo> {
        let creating = match origin {
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
                    self.provider.create(spec, origin),
                ),
            )
            .await?;
        machine_facts(boot, &machine, &spec.networks);
        Ok(machine)
    }

    /// The `created` (or `forked`) row of `machine`, made from `origin`,
    /// on `ledger`.
    fn created_row(&self, origin: &MachineOrigin, machine: &MachineInfo, ledger: &str) {
        let event = match origin {
            MachineOrigin::Fork(_) => VmEvent::Forked,
            _ => VmEvent::Created,
        };
        let source = match origin {
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
            ledger,
            &VmPayload {
                source: machine.source.clone().or(source),
                boot_ms: Some(machine.boot_ms),
                ..Self::base_row(event, machine)
            },
        );
    }

    /// Keep `machine` as `request`'s agent's, claiming.
    fn keep_claiming(&self, request: &ProvisionRequest, machine: &MachineInfo, ledger: &str) {
        self.machines.lock().insert(
            request.agent.clone(),
            Entry {
                owner: request.owner.clone(),
                plans: request.plans.clone(),
                git: request.git.clone(),
                project: request.project.clone(),
                ledger: ledger.to_string(),
                machine: machine.clone(),
                state: VmState::Claiming,
                busy: 0,
                idle_since: Instant::now(),
                node_token: None,
                node_protocol: 0,
                usage: None,
                seen_ms: now_ms(),
            },
        );
    }

    /// Set a new machine up as the [`BootStage::Configure`] stage: bot
    /// protection off and boxd's hibernation backstop on, each shown and
    /// never fatal.
    async fn configure(&self, machine: &MachineInfo, boot: &BootReporter) -> CloudResult<()> {
        boot.run(BootStage::Configure, async {
            self.bot_protection_off(machine, boot).await;
            self.hibernation_backstop(machine, boot).await;
            Ok::<_, CloudError>(())
        })
        .await
    }

    async fn bot_protection_off(&self, machine: &MachineInfo, boot: &BootReporter) {
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
            boot.log(
                BootStage::Configure,
                true,
                format!("bot protection stays on: {err}"),
            );
        } else {
            boot.log(
                BootStage::Configure,
                false,
                "bot protection off for the node API",
            );
        }
    }

    async fn hibernation_backstop(&self, machine: &MachineInfo, boot: &BootReporter) {
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
    }
}
