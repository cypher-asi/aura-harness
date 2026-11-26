//! [`VmProvider`] for [`FakeProvider`]: every call logged, answered
//! from the maps.

use std::time::Duration;

use async_trait::async_trait;
use aura_core_types::VmUsage;

use super::FakeProvider;
use crate::error::{CloudError, CloudResult};
use crate::provider::{
    Balance, ExecOutput, HostSecret, MachineInfo, MachineLimit, MachineOrigin, MachineStatus,
    NewMachine, OrgUsage, SnapshotInfo, UsageReading, VmProvider,
};

fn status_error(op: &'static str, message: &str) -> CloudError {
    CloudError::Rpc {
        op,
        code: "FailedPrecondition".into(),
        message: message.into(),
    }
}

#[async_trait]
impl VmProvider for FakeProvider {
    async fn create(&self, spec: &NewMachine, origin: &MachineOrigin) -> CloudResult<MachineInfo> {
        self.log(format!(
            "Create({}, {origin:?}, ttl={:?})",
            spec.name, spec.delete_after_secs
        ));
        let mut state = self.state.lock();
        state.check_create(spec)?;
        let basis = state.basis(spec, origin)?;
        Ok(state.add_machine(spec, basis))
    }

    async fn get(&self, machine: &str) -> CloudResult<Option<MachineInfo>> {
        self.log(format!("Get({machine})"));
        let state = self.state.lock();
        Ok(Self::find(&state, machine).and_then(|name| state.machines.get(&name).cloned()))
    }

    async fn list(&self, label_selector: &[String]) -> CloudResult<Vec<MachineInfo>> {
        self.log(format!("List({})", label_selector.join(",")));
        let state = self.state.lock();
        Ok(state
            .machines
            .values()
            .filter(|info| {
                label_selector
                    .iter()
                    .all(|selector| match selector.split_once('=') {
                        Some((key, value)) => info.labels.get(key).is_some_and(|v| v == value),
                        None => info.labels.contains_key(selector.as_str()),
                    })
            })
            .cloned()
            .collect())
    }

    async fn destroy(&self, machine: &str) -> CloudResult<()> {
        self.log(format!("Destroy({machine})"));
        let mut state = self.state.lock();
        if let Some(name) = Self::find(&state, machine) {
            if let Some(info) = state.machines.remove(&name) {
                state.retired.insert(info.id, name.clone());
            }
            state.files.retain(|(owner, _), _| *owner != name);
        }
        Ok(())
    }

    async fn hibernate(&self, machine: &str) -> CloudResult<()> {
        self.log(format!("Hibernate({machine})"));
        self.with_machine(machine, |info| {
            info.status = MachineStatus::Hibernated;
            Ok(())
        })
    }

    async fn wake(&self, machine: &str) -> CloudResult<()> {
        self.log(format!("Wake({machine})"));
        self.with_machine(machine, |info| {
            info.status = MachineStatus::Running;
            Ok(())
        })
    }

    async fn standby(&self, machine: &str) -> CloudResult<()> {
        self.log(format!("Standby({machine})"));
        self.with_machine(machine, |info| {
            if info.status != MachineStatus::Running {
                return Err(status_error("SuspendVm", "machine is not running"));
            }
            info.status = MachineStatus::Standby;
            Ok(())
        })
    }

    async fn set_auto_hibernate(&self, machine: &str, secs: u32) -> CloudResult<()> {
        self.log(format!("SetAutoHibernate({machine}, {secs})"));
        self.with_machine(machine, |_| Ok(()))
    }

    async fn set_bot_protection(&self, machine: &str, enabled: bool) -> CloudResult<()> {
        self.log(format!("SetBotProtection({machine}, {enabled})"));
        self.with_machine(machine, |_| Ok(()))
    }

    async fn exec(
        &self,
        machine: &str,
        command: &str,
        stdin: Option<Vec<u8>>,
        _timeout: Duration,
    ) -> CloudResult<ExecOutput> {
        self.log(format!("Exec({machine}, {command})"));
        let name = self.with_machine(machine, |info| {
            // Any connection through boxd wakes a sleeping machine.
            info.status = MachineStatus::Running;
            Ok(info.name.clone())
        })?;
        let handler = self.exec.lock().clone();
        Ok(handler.map_or_else(ExecOutput::default, |handler| {
            handler(&name, command, stdin.as_deref())
        }))
    }

    async fn upload(&self, machine: &str, path: &str, bytes: Vec<u8>) -> CloudResult<u64> {
        self.log(format!("Upload({machine}, {path})"));
        let name = self.with_machine(machine, |info| Ok(info.name.clone()))?;
        let len = bytes.len() as u64;
        self.state
            .lock()
            .files
            .insert((name, path.to_string()), bytes);
        Ok(len)
    }

    async fn download(&self, machine: &str, path: &str, max_bytes: u64) -> CloudResult<Vec<u8>> {
        self.log(format!("Download({machine}, {path})"));
        let name = self.with_machine(machine, |info| Ok(info.name.clone()))?;
        let bytes = self
            .state
            .lock()
            .files
            .get(&(name, path.to_string()))
            .cloned()
            .ok_or_else(|| CloudError::NotFound {
                what: "file",
                name: path.to_string(),
            })?;
        if bytes.len() as u64 > max_bytes {
            return Err(CloudError::InvalidResponse {
                op: "DownloadFileStream",
                message: format!("{path} is above the {max_bytes} byte limit"),
            });
        }
        Ok(bytes)
    }

    async fn save_snapshot(&self, machine: &str, name: &str) -> CloudResult<SnapshotInfo> {
        self.log(format!("SaveSnapshot({machine}, {name})"));
        let vcpu = self.with_machine(machine, |info| {
            if info.status != MachineStatus::Running {
                return Err(status_error("CreateSnapshot", "machine must be running"));
            }
            Ok(info.vcpu)
        })?;
        let mut state = self.state.lock();
        let version = state.snapshots.get(name).map_or(1, |s| s.version + 1);
        let snapshot = SnapshotInfo {
            id: format!("snap-{name}-{version}"),
            name: name.to_string(),
            version,
            status: "ready".into(),
            size_bytes: 6 << 30,
            vcpu,
        };
        state.snapshots.insert(name.to_string(), snapshot.clone());
        Ok(snapshot)
    }

    async fn snapshot(&self, name: &str) -> CloudResult<Option<SnapshotInfo>> {
        self.log(format!("Snapshot({name})"));
        Ok(self.state.lock().snapshots.get(name).cloned())
    }

    async fn delete_snapshot(&self, name: &str) -> CloudResult<()> {
        self.log(format!("DeleteSnapshot({name})"));
        self.state.lock().snapshots.remove(name);
        Ok(())
    }

    async fn machine_limit(&self) -> CloudResult<MachineLimit> {
        let state = self.state.lock();
        Ok(MachineLimit {
            count: u32::try_from(state.machines.len()).unwrap_or(u32::MAX),
            max: state.max_machines,
        })
    }

    async fn machine_usage(&self, machine: &str) -> CloudResult<UsageReading> {
        self.log(format!("Usage({machine})"));
        let state = self.state.lock();
        if state.deny_usage {
            return Err(CloudError::Rpc {
                op: "GetVmUsage",
                code: "PermissionDenied".into(),
                message: "org owners and admins only".into(),
            });
        }
        let name = Self::find(&state, machine)
            .or_else(|| state.retired.get(machine).cloned())
            .unwrap_or_else(|| machine.to_string());
        let mut usage = state.usage.get(&name).copied().unwrap_or_default();
        // A machine that still exists is never complete.
        if state.machines.contains_key(&name) {
            usage.complete = false;
        }
        Ok(UsageReading {
            usage,
            currency: "eur".into(),
        })
    }

    async fn org_usage(&self, _since_ms: u64) -> CloudResult<OrgUsage> {
        let state = self.state.lock();
        if state.deny_usage {
            return Err(CloudError::Rpc {
                op: "GetOrgUsage",
                code: "PermissionDenied".into(),
                message: "org owners and admins only".into(),
            });
        }
        let machines: Vec<(String, VmUsage)> = state
            .usage
            .iter()
            .map(|(name, usage)| (name.clone(), *usage))
            .collect();
        Ok(OrgUsage {
            total_cost_micro: machines.iter().map(|(_, u)| u.cost_micro).sum(),
            currency: "eur".into(),
            complete: false,
            machines,
        })
    }

    async fn set_secret(&self, secret: &HostSecret) -> CloudResult<()> {
        self.log(format!(
            "SetSecret({}, {})",
            secret.name,
            secret.domains.join(",")
        ));
        self.state
            .lock()
            .secrets
            .insert(secret.name.clone(), secret.clone());
        Ok(())
    }

    async fn remove_secret(&self, name: &str) -> CloudResult<()> {
        self.log(format!("RemoveSecret({name})"));
        self.state.lock().secrets.remove(name);
        Ok(())
    }

    async fn balance(&self) -> CloudResult<Balance> {
        Ok(Balance {
            currency: "eur".into(),
            balance_micro: 27_100_000,
        })
    }
}
