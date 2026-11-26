//! An in-memory [`VmProvider`](crate::VmProvider) for tests of the fleet
//! and the hub. It keeps machines and snapshots in maps, logs every call,
//! and answers `exec` through a handler the test sets.

mod create;
mod provider;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use aura_config::MachineSize;
use aura_core_types::VmUsage;
use parking_lot::Mutex;

use crate::error::{CloudError, CloudResult};
use crate::provider::{ExecOutput, HostSecret, MachineInfo, MachineStatus, SnapshotInfo};

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
    secrets: BTreeMap<String, HostSecret>,
    /// The secret names each machine was given, by machine name.
    machine_secrets: HashMap<String, Vec<String>>,
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
    /// The value of secret `name`, if set.
    #[must_use]
    pub fn secret_value(&self, name: &str) -> Option<String> {
        self.state
            .lock()
            .secrets
            .get(name)
            .map(|secret| secret.value.as_str().to_string())
    }

    /// Every secret set, by name.
    #[must_use]
    pub fn secrets(&self) -> BTreeMap<String, HostSecret> {
        self.state.lock().secrets.clone()
    }

    /// The secret names machine `name` was given.
    #[must_use]
    pub fn machine_secrets(&self, name: &str) -> Vec<String> {
        self.state
            .lock()
            .machine_secrets
            .get(name)
            .cloned()
            .unwrap_or_default()
    }

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

    /// Put a file on a machine, as a command run there would.
    pub fn put_file(&self, machine: &str, path: &str, bytes: Vec<u8>) {
        self.state
            .lock()
            .files
            .insert((machine.to_string(), path.to_string()), bytes);
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
