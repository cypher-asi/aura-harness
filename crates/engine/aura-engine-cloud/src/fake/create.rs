//! Making a machine in the fake, refused where boxd would refuse it.

use aura_config::MachineSize;
use aura_core_types::VmSource;

use super::{FakeProvider, State};
use crate::error::{CloudError, CloudResult};
use crate::provider::{MachineInfo, MachineOrigin, MachineStatus, NewMachine, MACHINE_DISK_BYTES};

/// What a new machine takes from what it is made from.
pub(super) struct Basis {
    vcpu: u32,
    source: VmSource,
    /// The secret names it lists.
    secrets: Vec<String>,
}

impl State {
    /// Refuse what boxd refuses before it makes anything: the org is
    /// full, the name is taken, or a secret named doesn't resolve.
    pub(super) fn check_create(&self, spec: &NewMachine) -> CloudResult<()> {
        if u32::try_from(self.machines.len()).unwrap_or(u32::MAX) >= self.max_machines {
            return Err(CloudError::MachineLimit {
                count: u32::try_from(self.machines.len()).unwrap_or(u32::MAX),
                max: self.max_machines,
            });
        }
        if self.machines.contains_key(&spec.name) {
            return Err(CloudError::Rpc {
                op: "CreateVm",
                code: "AlreadyExists".into(),
                message: format!("name '{}' is already taken", spec.name),
            });
        }
        // boxd fails a create naming a secret that doesn't resolve.
        if let Some(missing) = spec
            .secrets
            .iter()
            .find(|name| !self.secrets.contains_key(*name))
        {
            return Err(CloudError::Rpc {
                op: "CreateVm",
                code: "FailedPrecondition".into(),
                message: format!("secret {missing} does not exist"),
            });
        }
        Ok(())
    }

    /// What `spec`'s machine takes from `origin`. A fork also gets its
    /// parent's files.
    pub(super) fn basis(
        &mut self,
        spec: &NewMachine,
        origin: &MachineOrigin,
    ) -> CloudResult<Basis> {
        match origin {
            MachineOrigin::Fresh => self.fresh_basis(spec),
            MachineOrigin::Snapshot(name) => self.snapshot_basis(spec, name),
            MachineOrigin::Fork(parent) => self.fork_basis(spec, parent),
        }
    }

    fn fresh_basis(&self, spec: &NewMachine) -> CloudResult<Basis> {
        if let Some(largest) = self.largest_size.filter(|largest| spec.size > *largest) {
            return Err(CloudError::Rpc {
                op: "CreateVm",
                code: "InvalidArgument".into(),
                message: format!(
                    "size {} is above the org quota ceiling {largest}",
                    spec.size
                ),
            });
        }
        Ok(Basis {
            vcpu: spec.size.vcpu(),
            source: VmSource::Image {
                image: String::new(),
            },
            secrets: spec.secrets.clone(),
        })
    }

    fn snapshot_basis(&self, spec: &NewMachine, name: &str) -> CloudResult<Basis> {
        let snapshot = self
            .snapshots
            .get(name)
            .cloned()
            .ok_or_else(|| CloudError::NotFound {
                what: "snapshot",
                name: name.to_string(),
            })?;
        Ok(Basis {
            vcpu: snapshot.vcpu,
            source: VmSource::Snapshot {
                name: name.to_string(),
                version: u32::try_from(snapshot.version).ok(),
            },
            secrets: spec.secrets.clone(),
        })
    }

    fn fork_basis(&mut self, spec: &NewMachine, parent: &str) -> CloudResult<Basis> {
        let parent_name = FakeProvider::find(self, parent).ok_or_else(|| CloudError::NotFound {
            what: "machine",
            name: parent.to_string(),
        })?;
        let parent_info =
            self.machines
                .get(&parent_name)
                .cloned()
                .ok_or_else(|| CloudError::NotFound {
                    what: "machine",
                    name: parent.to_string(),
                })?;
        // A fork copies the parent's files.
        let copied: Vec<_> = self
            .files
            .iter()
            .filter(|((machine, _), _)| *machine == parent_name)
            .map(|((_, path), bytes)| ((spec.name.clone(), path.clone()), bytes.clone()))
            .collect();
        self.files.extend(copied);
        // A fork inherits its parent's secret names.
        let secrets = self
            .machine_secrets
            .get(&parent_name)
            .cloned()
            .unwrap_or_default();
        Ok(Basis {
            vcpu: parent_info.vcpu,
            source: VmSource::Fork {
                parent: parent_name,
            },
            secrets,
        })
    }

    /// Add `spec`'s machine, running, made from `basis`.
    pub(super) fn add_machine(&mut self, spec: &NewMachine, basis: Basis) -> MachineInfo {
        self.next_id += 1;
        let size = MachineSize::from_vcpu(basis.vcpu).unwrap_or_default();
        let info = MachineInfo {
            id: format!("vm-{}", self.next_id),
            name: spec.name.clone(),
            status: MachineStatus::Running,
            region: String::new(),
            vcpu: size.vcpu(),
            memory_bytes: size.memory_bytes(),
            disk_bytes: MACHINE_DISK_BYTES,
            domain: format!("{}.boxd.sh", spec.name),
            public_ip: "203.0.113.7".into(),
            ssh_port: 22_000,
            isolated: spec.isolated,
            labels: spec.labels.clone(),
            source: Some(basis.source),
            created_ms: 1,
            boot_ms: 6,
        };
        self.machines.insert(info.name.clone(), info.clone());
        self.machine_secrets
            .insert(info.name.clone(), basis.secrets);
        info
    }
}
