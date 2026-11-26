//! The cloud provider seam: what the fleet needs from a machine host,
//! in boxd's terms but without its wire types. [`crate::boxd`] is the
//! real implementation, [`crate::fake`] the one tests drive.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use aura_config::MachineSize;
use aura_core_types::{VmSource, VmUsage};
use serde::{Deserialize, Serialize};

use crate::error::CloudResult;

/// boxd's fixed disk size per machine (copy-on-write).
pub const MACHINE_DISK_BYTES: u64 = 100 * 1024 * 1024 * 1024;

/// A machine's lifecycle state, as boxd reports it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineStatus {
    /// Being created or booted.
    Creating,
    /// Running.
    Running,
    /// On standby: memory kept in RAM (boxd's API says `suspended`).
    Standby,
    /// Hibernated: memory on disk, disk-only billing.
    Hibernated,
    /// Shut down; memory lost, disk kept.
    Stopped,
    /// In an error state.
    Error,
    /// A status this build does not know.
    Other(String),
}

impl MachineStatus {
    /// Parse boxd's status string.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "creating" | "starting" | "booting" | "pending" | "provisioning" | "restoring"
            | "waking" | "resuming" => Self::Creating,
            "running" | "ready" => Self::Running,
            "suspended" | "standby" | "paused" => Self::Standby,
            "hibernated" | "hibernating" => Self::Hibernated,
            "stopped" | "stopping" => Self::Stopped,
            "error" | "failed" => Self::Error,
            other => Self::Other(other.to_string()),
        }
    }

    /// The status as shown and stored.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Creating => "creating",
            Self::Running => "running",
            Self::Standby => "standby",
            Self::Hibernated => "hibernated",
            Self::Stopped => "stopped",
            Self::Error => "error",
            Self::Other(raw) => raw,
        }
    }

    /// Whether the machine is asleep (standby or hibernated).
    #[must_use]
    pub const fn is_asleep(&self) -> bool {
        matches!(self, Self::Standby | Self::Hibernated)
    }
}

/// One machine as the provider sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineInfo {
    /// boxd machine id.
    pub id: String,
    /// Machine name; unique in the org.
    pub name: String,
    /// Lifecycle state.
    pub status: MachineStatus,
    /// Region it lives in (empty = the home region).
    pub region: String,
    /// vCPUs.
    pub vcpu: u32,
    /// Memory, bytes.
    pub memory_bytes: u64,
    /// Disk, bytes.
    pub disk_bytes: u64,
    /// HTTPS domain (`<name>.boxd.sh`).
    pub domain: String,
    /// Public IP of the proxy it is reached through.
    pub public_ip: String,
    /// SSH port on that proxy (0 = none).
    pub ssh_port: u32,
    /// Whether it was created isolated.
    pub isolated: bool,
    /// Its labels.
    pub labels: BTreeMap<String, String>,
    /// What it was made from.
    pub source: Option<VmSource>,
    /// When it was created, Unix milliseconds.
    pub created_ms: u64,
    /// How long it took to come up, milliseconds.
    pub boot_ms: u64,
}

impl MachineInfo {
    /// The machine's HTTPS base URL.
    #[must_use]
    pub fn url(&self) -> String {
        if self.domain.starts_with("http://") || self.domain.starts_with("https://") {
            self.domain.clone()
        } else {
            format!("https://{}", self.domain)
        }
    }

    /// Its size, when it is one boxd offers.
    #[must_use]
    pub fn size(&self) -> Option<MachineSize> {
        MachineSize::from_vcpu(self.vcpu)
    }
}

/// What to make a machine from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineOrigin {
    /// A fresh boot of boxd's default image, with a spec's packages.
    Fresh,
    /// Restored from the named snapshot (memory and disk).
    Snapshot(String),
    /// A fork of the machine with this name or id.
    Fork(String),
}

/// A machine to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMachine {
    /// Name; becomes `<name>.boxd.sh`.
    pub name: String,
    /// Size, for fresh machines (restores and forks keep their source's).
    pub size: MachineSize,
    /// Create isolated: no in-machine boxd CLI, no shared network.
    pub isolated: bool,
    /// Labels (`aura.install`, `aura.agent`, `aura.project`).
    pub labels: BTreeMap<String, String>,
    /// Environment for the machine (an allowlist; never `BOXD_*`).
    pub env: BTreeMap<String, String>,
    /// First-boot shell commands, for fresh machines.
    pub packages: Vec<String>,
    /// Networks to join (non-isolated machines only).
    pub networks: Vec<String>,
    /// Delete the machine this long after creation.
    pub delete_after_secs: Option<u64>,
    /// boxd secrets the machine gets by name (unattached secrets set with
    /// [`VmProvider::set_secret`]): its owner's plan secrets, nothing else.
    /// Forks inherit their parent's.
    pub secrets: Vec<String>,
}

impl NewMachine {
    /// A machine called `name` with every other field defaulted.
    #[must_use]
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            size: MachineSize::default(),
            isolated: true,
            labels: BTreeMap::new(),
            env: BTreeMap::new(),
            packages: Vec::new(),
            networks: Vec::new(),
            delete_after_secs: None,
            secrets: Vec::new(),
        }
    }
}

/// What a command run in a machine produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExecOutput {
    /// Exit code.
    pub exit_code: i32,
    /// Standard output.
    pub stdout: Vec<u8>,
    /// Standard error.
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    /// Whether it exited 0.
    #[must_use]
    pub const fn success(&self) -> bool {
        self.exit_code == 0
    }

    /// Stdout as text, lossily.
    #[must_use]
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// The last `max` bytes of stderr as text, for error messages.
    #[must_use]
    pub fn stderr_tail(&self, max: usize) -> String {
        let start = self.stderr.len().saturating_sub(max);
        String::from_utf8_lossy(&self.stderr[start..])
            .trim()
            .to_string()
    }
}

/// A snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotInfo {
    /// Snapshot id.
    pub id: String,
    /// Name; saving over it bumps the version.
    pub name: String,
    /// Latest version.
    pub version: u64,
    /// `ready`, `pending`, ...
    pub status: String,
    /// Size, bytes.
    pub size_bytes: u64,
    /// vCPUs machines made from it get.
    pub vcpu: u32,
}

impl SnapshotInfo {
    /// Whether machines can be created from it.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.status.eq_ignore_ascii_case("ready")
    }
}

/// A machine's metered usage, in the org's currency.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageReading {
    /// The reading.
    pub usage: VmUsage,
    /// `eur` or `usd`.
    pub currency: String,
}

/// The org's usage over a window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrgUsage {
    /// Total cost, micro-units.
    pub total_cost_micro: i64,
    /// `eur` or `usd`.
    pub currency: String,
    /// Whether every second of the window is metered.
    pub complete: bool,
    /// Per machine, by name.
    pub machines: Vec<(String, VmUsage)>,
}

/// How many machines the org may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineLimit {
    /// Machines in use, hibernated ones included.
    pub count: u32,
    /// The limit.
    pub max: u32,
}

impl MachineLimit {
    /// Whether another machine fits.
    #[must_use]
    pub const fn has_room(&self) -> bool {
        self.max == 0 || self.count < self.max
    }
}

/// The org's credit balance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Balance {
    /// `eur` or `usd`.
    pub currency: String,
    /// Balance, micro-units.
    pub balance_micro: i64,
}

/// A machine host. Every call has its own timeout inside the
/// implementation (rules.md §6.2).
/// A boxd secret bound to the hosts it may be sent to: machines listing
/// it hold only a `bxds_` placeholder, and boxd's egress proxy puts the
/// value into requests to `domains`.
#[derive(Clone, PartialEq, Eq)]
pub struct HostSecret {
    /// Secret name.
    pub name: String,
    /// Its value; never logged.
    pub value: zeroize::Zeroizing<String>,
    /// Hosts it may be sent to (`api.anthropic.com`).
    pub domains: Vec<String>,
    /// Parts of a request it is replaced in (`headers`); empty = all.
    pub replace_in: Vec<String>,
}

impl std::fmt::Debug for HostSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostSecret")
            .field("name", &self.name)
            .field("domains", &self.domains)
            .field("replace_in", &self.replace_in)
            .finish_non_exhaustive()
    }
}

#[async_trait]
pub trait VmProvider: Send + Sync {
    /// Create a machine from `origin`.
    async fn create(&self, spec: &NewMachine, origin: &MachineOrigin) -> CloudResult<MachineInfo>;
    /// A machine by name or id; `None` when it is gone.
    async fn get(&self, machine: &str) -> CloudResult<Option<MachineInfo>>;
    /// Machines whose labels match every `key=value` selector.
    async fn list(&self, label_selector: &[String]) -> CloudResult<Vec<MachineInfo>>;
    /// Destroy a machine. A machine that is already gone is not an error.
    async fn destroy(&self, machine: &str) -> CloudResult<()>;
    /// Hibernate a machine (memory to disk).
    async fn hibernate(&self, machine: &str) -> CloudResult<()>;
    /// Wake a machine from standby or hibernation.
    async fn wake(&self, machine: &str) -> CloudResult<()>;
    /// Put a running machine on standby (memory kept in RAM).
    async fn standby(&self, machine: &str) -> CloudResult<()>;
    /// Set boxd's own idle hibernation backstop; `0` turns it off.
    async fn set_auto_hibernate(&self, machine: &str, secs: u32) -> CloudResult<()>;
    /// Turn boxd's bot protection on or off for the machine's domain.
    async fn set_bot_protection(&self, machine: &str, enabled: bool) -> CloudResult<()>;
    /// Run `command` in the machine's shell with optional stdin.
    async fn exec(
        &self,
        machine: &str,
        command: &str,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
    ) -> CloudResult<ExecOutput>;
    /// [`Self::exec`], handing each piece of output to `on_chunk`
    /// (`true` for stderr) as it arrives. By default the whole output is
    /// handed over once the command ends.
    async fn exec_streaming(
        &self,
        machine: &str,
        command: &str,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        on_chunk: &mut (dyn for<'chunk> FnMut(bool, &'chunk [u8]) + Send),
    ) -> CloudResult<ExecOutput> {
        let output = self.exec(machine, command, stdin, timeout).await?;
        on_chunk(false, &output.stdout);
        on_chunk(true, &output.stderr);
        Ok(output)
    }
    /// Write `bytes` to `path` in the machine.
    async fn upload(&self, machine: &str, path: &str, bytes: Vec<u8>) -> CloudResult<u64>;
    /// [`Self::upload`], calling `on_sent(sent, total)` now and then as
    /// the bytes go out. By default it is called once, when all are sent.
    async fn upload_reported(
        &self,
        machine: &str,
        path: &str,
        bytes: Vec<u8>,
        on_sent: &mut (dyn FnMut(u64, u64) + Send),
    ) -> CloudResult<u64> {
        let total = bytes.len() as u64;
        let written = self.upload(machine, path, bytes).await?;
        on_sent(total, total);
        Ok(written)
    }
    /// Read `path` from the machine, at most `max_bytes`.
    async fn download(&self, machine: &str, path: &str, max_bytes: u64) -> CloudResult<Vec<u8>>;
    /// Save a snapshot of a running machine under `name`.
    async fn save_snapshot(&self, machine: &str, name: &str) -> CloudResult<SnapshotInfo>;
    /// A snapshot by name; `None` when there is none.
    async fn snapshot(&self, name: &str) -> CloudResult<Option<SnapshotInfo>>;
    /// Delete a snapshot.
    async fn delete_snapshot(&self, name: &str) -> CloudResult<()>;
    /// The org's machine limit.
    async fn machine_limit(&self) -> CloudResult<MachineLimit>;
    /// A machine's metered usage over its life (works after deletion).
    async fn machine_usage(&self, machine: &str) -> CloudResult<UsageReading>;
    /// The org's usage since `since_ms`.
    async fn org_usage(&self, since_ms: u64) -> CloudResult<OrgUsage>;
    /// Create or rotate an unattached, host-bound secret. Rotating keeps
    /// the placeholder machines hold.
    async fn set_secret(&self, secret: &HostSecret) -> CloudResult<()>;
    /// Remove a secret; one that is gone is not an error.
    async fn remove_secret(&self, name: &str) -> CloudResult<()>;
    /// The org's credit balance.
    async fn balance(&self) -> CloudResult<Balance>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_parse_boxd_names() {
        assert_eq!(MachineStatus::parse("suspended"), MachineStatus::Standby);
        assert_eq!(
            MachineStatus::parse("Hibernated"),
            MachineStatus::Hibernated
        );
        assert_eq!(MachineStatus::parse("running"), MachineStatus::Running);
        assert_eq!(
            MachineStatus::parse("weird"),
            MachineStatus::Other("weird".into())
        );
        assert!(MachineStatus::Standby.is_asleep());
        assert!(!MachineStatus::Running.is_asleep());
    }

    #[test]
    fn urls_get_a_scheme() {
        let info = MachineInfo {
            id: "1".into(),
            name: "m".into(),
            status: MachineStatus::Running,
            region: String::new(),
            vcpu: 4,
            memory_bytes: 0,
            disk_bytes: 0,
            domain: "m.boxd.sh".into(),
            public_ip: String::new(),
            ssh_port: 0,
            isolated: true,
            labels: BTreeMap::new(),
            source: None,
            created_ms: 0,
            boot_ms: 0,
        };
        assert_eq!(info.url(), "https://m.boxd.sh");
        assert_eq!(info.size(), Some(MachineSize::S4x16));
    }

    #[test]
    fn limits_and_exec_output() {
        assert!(MachineLimit { count: 1, max: 2 }.has_room());
        assert!(!MachineLimit { count: 2, max: 2 }.has_room());
        let out = ExecOutput {
            exit_code: 1,
            stdout: b"ok".to_vec(),
            stderr: b"0123456789".to_vec(),
        };
        assert_eq!(out.stderr_tail(4), "6789");
        assert!(!out.success());
    }
}
