//! An in-memory [`VmProvider`] for tests of the fleet and the hub. It
//! keeps machines and snapshots in maps, logs every call, and answers
//! `exec` through a handler the test sets.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aura_config::MachineSize;
use aura_core_types::{VmSource, VmUsage};
use parking_lot::Mutex;

use crate::error::{CloudError, CloudResult};
use crate::provider::{
    Balance, ExecOutput, MachineInfo, MachineLimit, MachineOrigin, MachineStatus, NewMachine,
    OrgUsage, SnapshotInfo, UsageReading, VmProvider, MACHINE_DISK_BYTES,
};

/// Answers an `exec`: machine name, command and stdin to output.
pub type ExecHandler = Arc<dyn Fn(&str, &str, Option<&[u8]>) -> ExecOutput + Send + Sync>;

#[derive(Default)]
struct State {
    machines: BTreeMap<String, MachineInfo>,
    snapshots: HashMap<String, SnapshotInfo>,
    files: HashMap<(String, String), Vec<u8>>,
    usage: HashMap<String, VmUsage>,
    /// Names of destroyed machines by id: usage stays readable by id.
    retired: HashMap<String, String>,
    calls: Vec<String>,
    next_id: u64,
    max_machines: u32,
    largest_size: Option<MachineSize>,
    deny_usage: bool,
}

/// The fake.
#[derive(Clone)]
pub struct FakeProvider {
    state: Arc<Mutex<State>>,
    exec: Arc<Mutex<Option<ExecHandler>>>,
}

impl Default for FakeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeProvider {
    /// An empty fake with room for 50 machines and every size.
    #[must_use]
    pub fn new() -> Self {
        let state = State {
            max_machines: 50,
            ..State::default()
        };
        Self {
            state: Arc::new(Mutex::new(state)),
            exec: Arc::new(Mutex::new(None)),
        }
    }

    /// Answer `exec` with `handler` (default: exit 0, no output).
    pub fn on_exec(&self, handler: ExecHandler) {
        *self.exec.lock() = Some(handler);
    }

    /// Allow at most `max` machines.
    pub fn set_machine_limit(&self, max: u32) {
        self.state.lock().max_machines = max;
    }

    /// Refuse sizes above `largest`, as an org quota does.
    pub fn set_largest_size(&self, largest: MachineSize) {
        self.state.lock().largest_size = Some(largest);
    }

    /// Answer usage calls with `PERMISSION_DENIED` (a member key).
    pub fn deny_usage(&self) {
        self.state.lock().deny_usage = true;
    }

    /// Set a machine's metered usage.
    pub fn set_usage(&self, machine: &str, usage: VmUsage) {
        self.state.lock().usage.insert(machine.to_string(), usage);
    }

    /// Set a machine's status, as boxd's own timers would.
    pub fn set_status(&self, machine: &str, status: MachineStatus) {
        if let Some(info) = self.state.lock().machines.get_mut(machine) {
            info.status = status;
        }
    }

    /// Add a machine as if made elsewhere (for orphan sweeps).
    pub fn insert_machine(&self, info: MachineInfo) {
        self.state.lock().machines.insert(info.name.clone(), info);
    }

    /// Every call so far, as `Op(machine)`.
    #[must_use]
    pub fn calls(&self) -> Vec<String> {
        self.state.lock().calls.clone()
    }

    /// Calls whose text starts with `prefix`.
    #[must_use]
    pub fn calls_matching(&self, prefix: &str) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|call| call.starts_with(prefix))
            .collect()
    }

    /// A machine by name.
    #[must_use]
    pub fn machine(&self, name: &str) -> Option<MachineInfo> {
        self.state.lock().machines.get(name).cloned()
    }

    /// Names of every machine.
    #[must_use]
    pub fn machine_names(&self) -> Vec<String> {
        self.state.lock().machines.keys().cloned().collect()
    }

    /// A file uploaded to a machine.
    #[must_use]
    pub fn file(&self, machine: &str, path: &str) -> Option<Vec<u8>> {
        self.state
            .lock()
            .files
            .get(&(machine.to_string(), path.to_string()))
            .cloned()
    }

    fn log(&self, call: String) {
        self.state.lock().calls.push(call);
    }

    fn find(state: &State, machine: &str) -> Option<String> {
        if state.machines.contains_key(machine) {
            return Some(machine.to_string());
        }
        state
            .machines
            .values()
            .find(|info| info.id == machine)
            .map(|info| info.name.clone())
    }

    fn with_machine<T>(
        &self,
        machine: &str,
        f: impl FnOnce(&mut MachineInfo) -> CloudResult<T>,
    ) -> CloudResult<T> {
        let mut state = self.state.lock();
        let name = Self::find(&state, machine).ok_or_else(|| CloudError::NotFound {
            what: "machine",
            name: machine.to_string(),
        })?;
        let info = state
            .machines
            .get_mut(&name)
            .ok_or_else(|| CloudError::NotFound {
                what: "machine",
                name: machine.to_string(),
            })?;
        f(info)
    }
}

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
        if u32::try_from(state.machines.len()).unwrap_or(u32::MAX) >= state.max_machines {
            return Err(CloudError::MachineLimit {
                count: u32::try_from(state.machines.len()).unwrap_or(u32::MAX),
                max: state.max_machines,
            });
        }
        if state.machines.contains_key(&spec.name) {
            return Err(CloudError::Rpc {
                op: "CreateVm",
                code: "AlreadyExists".into(),
                message: format!("name '{}' is already taken", spec.name),
            });
        }
        let (vcpu, source) = match origin {
            MachineOrigin::Fresh => {
                if let Some(largest) = state.largest_size.filter(|largest| spec.size > *largest) {
                    return Err(CloudError::Rpc {
                        op: "CreateVm",
                        code: "InvalidArgument".into(),
                        message: format!(
                            "size {} is above the org quota ceiling {largest}",
                            spec.size
                        ),
                    });
                }
                (
                    spec.size.vcpu(),
                    VmSource::Image {
                        image: String::new(),
                    },
                )
            }
            MachineOrigin::Snapshot(name) => {
                let snapshot =
                    state
                        .snapshots
                        .get(name)
                        .cloned()
                        .ok_or_else(|| CloudError::NotFound {
                            what: "snapshot",
                            name: name.clone(),
                        })?;
                (
                    snapshot.vcpu,
                    VmSource::Snapshot {
                        name: name.clone(),
                        version: u32::try_from(snapshot.version).ok(),
                    },
                )
            }
            MachineOrigin::Fork(parent) => {
                let parent_name =
                    Self::find(&state, parent).ok_or_else(|| CloudError::NotFound {
                        what: "machine",
                        name: parent.clone(),
                    })?;
                let parent_info = state.machines.get(&parent_name).cloned().ok_or_else(|| {
                    CloudError::NotFound {
                        what: "machine",
                        name: parent.clone(),
                    }
                })?;
                // A fork copies the parent's files.
                let copied: Vec<_> = state
                    .files
                    .iter()
                    .filter(|((machine, _), _)| *machine == parent_name)
                    .map(|((_, path), bytes)| ((spec.name.clone(), path.clone()), bytes.clone()))
                    .collect();
                state.files.extend(copied);
                (
                    parent_info.vcpu,
                    VmSource::Fork {
                        parent: parent_name,
                    },
                )
            }
        };
        state.next_id += 1;
        let size = MachineSize::from_vcpu(vcpu).unwrap_or_default();
        let info = MachineInfo {
            id: format!("vm-{}", state.next_id),
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
            source: Some(source),
            created_ms: 1,
            boot_ms: 6,
        };
        state.machines.insert(info.name.clone(), info.clone());
        Ok(info)
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

    async fn balance(&self) -> CloudResult<Balance> {
        Ok(Balance {
            currency: "eur".into(),
            balance_micro: 27_100_000,
        })
    }
}
