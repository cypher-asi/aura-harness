//! Snapshots that make cloud agents start warm.
//!
//! boxd snapshots capture memory and disk, so a machine restored from
//! one resumes with whatever the builder was running: the toolchain
//! installed, the project built, and the aura node already listening
//! (unclaimed, until the hub claims it).
//!
//! | Snapshot | Built from | Holds |
//! |---|---|---|
//! | [`BASE_SNAPSHOT`](names::BASE_SNAPSHOT) | a fresh machine | build tools, rustup, the aura binary |
//! | `aura-proj-<project>` | the base | the project at a commit, fetched and built |
//!
//! A snapshot is rebuilt when its fingerprint changes: the bootstrap
//! script, the aura version, the machine size, the toolchain file, the
//! lockfiles or the build command. boxd keeps only the latest version of
//! a name, so one name per project is enough.
//!
//! ## Failure modes
//!
//! A builder that fails is destroyed and the error recorded on the
//! project's ledger; the previous snapshot (if any) stays in use.
//! Builders also carry a delete-after of [`BUILDER_TTL_SECS`], so a hub
//! that dies mid-build leaves nothing running for long.

mod binary;
mod build;
mod ensure;
mod fingerprint;
mod index;

pub use self::binary::{binary_digest, cloud_binary_path, CHECK_BINARY, CHECK_PROTOCOL};
pub use self::build::uploading;
pub use self::fingerprint::{base_fingerprint, default_build_command, project_fingerprint};
pub use self::index::{SnapshotIndex, SnapshotRecord};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aura_config::MachineSize;
use tokio::sync::Mutex;

use crate::error::CloudResult;
use crate::fleet::VmLedger;
use crate::names;
use crate::provider::VmProvider;

/// Bump when [`base_packages`] or the node start changes.
pub const BOOTSTRAP_VERSION: &str = "4";
/// Where a machine finds out which snapshot it came from.
pub const SNAPSHOT_MANIFEST: &str = "/etc/aura/snapshot.json";
/// Builders delete themselves after this long, seconds.
pub const BUILDER_TTL_SECS: u64 = 2 * 60 * 60;
/// Limit on one build step.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(90 * 60);
/// Limit on short steps.
pub const SHORT_STEP_TIMEOUT: Duration = Duration::from_secs(120);
/// How long to wait for a saved snapshot to become ready.
pub const READY_WAIT: Duration = Duration::from_secs(10 * 60);

/// Stops a running node and waits for it to exit: a machine restored
/// from a snapshot comes back with the snapshot's node running, and a
/// new node can't open the store until the old one lets go of it.
/// `-x` matches by process name; `-f` would match this shell too.
pub const STOP_NODE: &str = "pkill -x aura || true; \
for i in $(seq 1 30); do pgrep -x aura >/dev/null || break; sleep 1; done; \
pkill -9 -x aura || true; while pgrep -x aura >/dev/null; do sleep 1; done";

/// Starts the node unclaimed, in the background, with the machine env.
pub const START_NODE: &str = "set -e; sudo mkdir -p /var/log/aura; sudo chown \"$(id -u)\" /var/log/aura; \
set -a; . /etc/aura/machine.env; set +a; \
nohup aura run --ui none > /var/log/aura/node.log 2>&1 < /dev/null & \
for i in $(seq 1 60); do curl -fsS \"http://127.0.0.1:${AURA_LISTEN_ADDR##*:}/health\" >/dev/null 2>&1 && exit 0; sleep 1; done; \
echo 'aura node did not come up' >&2; tail -n 40 /var/log/aura/node.log >&2; exit 1";

/// Setup commands of the base builder, run in order as the machine's
/// user (with `sudo` where root is needed), so the toolchains land in
/// that user's home.
#[must_use]
pub fn base_packages() -> Vec<String> {
    vec![
        "sudo apt-get update -y".into(),
        "sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
         build-essential clang mold cmake pkg-config libssl-dev libclang-dev git curl ca-certificates \
         python3 nodejs npm libdbus-1-3 libdbus-1-dev libssl3"
            .into(),
        "curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
         --component rustfmt --component clippy"
            .into(),
        "sudo ln -sf \"$HOME/.cargo/bin/\"* /usr/local/bin/ && cargo --version".into(),
        // GitHub's CLI, from GitHub's own apt repository, for pull requests.
        "sudo mkdir -p -m 755 /etc/apt/keyrings \
         && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
            | sudo tee /etc/apt/keyrings/githubcli-archive-keyring.gpg > /dev/null \
         && sudo chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg \
         && echo \"deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] \
            https://cli.github.com/packages stable main\" \
            | sudo tee /etc/apt/sources.list.d/github-cli.list > /dev/null \
         && sudo apt-get update -y \
         && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends gh \
         && gh --version"
            .into(),
        format!(
            "sudo mkdir -p /etc/aura {ws} {data} && sudo chown \"$(id -u)\" {ws} {data}",
            ws = crate::machine_env::WORKSPACE_DIR,
            data = crate::machine_env::DATA_DIR
        ),
    ]
}

/// The aura binary the base snapshot gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuraBinary {
    /// Upload this file (a Linux build matching the machines' arch).
    Upload(PathBuf),
    /// The base packages already install it (tests).
    Installed,
}

/// What a [`SnapshotBuilder`] builds snapshots for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSettings {
    /// This install's id, on every builder machine's install label.
    pub install: String,
    /// Size of the builder machines, part of the base fingerprint.
    pub size: MachineSize,
    /// Whether builder machines are created isolated.
    pub isolated: bool,
    /// The aura version, part of the base fingerprint.
    pub aura_version: String,
    /// The aura binary the base snapshot gets.
    pub binary: AuraBinary,
}

/// Builds and reuses snapshots.
pub struct SnapshotBuilder {
    provider: Arc<dyn VmProvider>,
    ledger: Arc<dyn VmLedger>,
    install: String,
    size: MachineSize,
    isolated: bool,
    aura_version: String,
    binary: AuraBinary,
    /// The cloud home (`AURA_HOME/cloud`).
    home: PathBuf,
    index_path: PathBuf,
    /// The snapshot index; also guards the index file.
    index: Mutex<Option<SnapshotIndex>>,
    /// One gate per snapshot (the base, each project): a start that
    /// needs a snapshot another start is building waits for that build
    /// and reuses it, so agents launched together trigger one build.
    gates: parking_lot::Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl SnapshotBuilder {
    /// A builder keeping its index in `home` (`AURA_HOME/cloud`), where
    /// [`cloud_binary_path`] also lives.
    #[must_use]
    pub fn new(
        provider: Arc<dyn VmProvider>,
        ledger: Arc<dyn VmLedger>,
        settings: SnapshotSettings,
        home: PathBuf,
    ) -> Self {
        let SnapshotSettings {
            install,
            size,
            isolated,
            aura_version,
            binary,
        } = settings;
        Self {
            provider,
            ledger,
            install,
            size,
            isolated,
            aura_version,
            binary,
            index_path: home.join("snapshots.json"),
            home,
            index: Mutex::new(None),
            gates: parking_lot::Mutex::default(),
        }
    }

    /// The gate of snapshot `key` ([`Self::gates`]).
    fn gate(&self, key: &str) -> Arc<Mutex<()>> {
        Arc::clone(self.gates.lock().entry(key.to_string()).or_default())
    }

    /// Delete a project's snapshot and forget it.
    ///
    /// # Errors
    ///
    /// When boxd refuses the delete.
    pub async fn remove_project(&self, project_id: &str) -> CloudResult<()> {
        self.provider
            .delete_snapshot(&names::project_snapshot(project_id))
            .await?;
        let mut guard = self.index.lock().await;
        let index = guard.get_or_insert_with(SnapshotIndex::default);
        index.records.remove(project_id);
        index.save(&self.index_path).await
    }
}

#[cfg(test)]
mod build_once_tests;
#[cfg(test)]
mod tests;
