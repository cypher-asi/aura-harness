//! Snapshots that make cloud agents start warm.
//!
//! boxd snapshots capture memory and disk, so a machine restored from
//! one resumes with whatever the builder was running: the toolchain
//! installed, the project built, and the aura node already listening
//! (unclaimed, until the hub claims it).
//!
//! | Snapshot | Built from | Holds |
//! |---|---|---|
//! | [`BASE_SNAPSHOT`] | a fresh machine | build tools, rustup, the aura binary |
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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aura_config::MachineSize;
use aura_core_protocol::cloud::SnapshotOrigin;
use aura_core_types::{VmEvent, VmPayload};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::delta;
use crate::error::{CloudError, CloudResult};
use crate::fleet::VmLedger;
use crate::machine_env::MachineEnv;
use crate::names::{self, BASE_SNAPSHOT, LABEL_INSTALL, LABEL_PROJECT, LABEL_ROLE, ROLE_BUILDER};
use crate::provider::{MachineOrigin, NewMachine, VmProvider};

/// Bump when [`base_packages`] or the node start changes.
pub const BOOTSTRAP_VERSION: &str = "2";
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
         python3 nodejs npm libdbus-1-3 libssl3"
            .into(),
        "curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
         --component rustfmt --component clippy"
            .into(),
        "sudo ln -sf \"$HOME/.cargo/bin/\"* /usr/local/bin/ && cargo --version".into(),
        format!(
            "sudo mkdir -p /etc/aura {ws} {data} && sudo chown \"$(id -u)\" {ws} {data}",
            ws = crate::machine_env::WORKSPACE_DIR,
            data = crate::machine_env::DATA_DIR
        ),
    ]
}

/// Files whose contents decide a project snapshot.
const FINGERPRINT_FILES: &[&str] = &[
    "rust-toolchain.toml",
    "rust-toolchain",
    "Cargo.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lockb",
    "go.sum",
    "uv.lock",
    "poetry.lock",
    "requirements.txt",
];

/// Fingerprint of the base snapshot.
#[must_use]
pub fn base_fingerprint(size: MachineSize, aura_version: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in [BOOTSTRAP_VERSION, aura_version, size.as_spec()] {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    hasher.finalize().to_hex()[..16].to_string()
}

/// Fingerprint of a project snapshot. Reads the lockfiles (blocking).
#[must_use]
pub fn project_fingerprint(project: &Path, base: &str, build_command: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in [base, build_command] {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    for file in FINGERPRINT_FILES {
        hasher.update(file.as_bytes());
        match std::fs::read(project.join(file)) {
            Ok(bytes) => {
                hasher.update(&[1]);
                hasher.update(&bytes);
            }
            Err(_) => {
                hasher.update(&[0]);
            }
        }
    }
    hasher.finalize().to_hex()[..16].to_string()
}

/// The build a project gets when none is configured.
#[must_use]
pub fn default_build_command(project: &Path) -> String {
    if project.join("Cargo.toml").is_file() {
        "cargo fetch && cargo test --workspace --no-run".into()
    } else if project.join("package-lock.json").is_file() {
        "npm ci".into()
    } else if project.join("package.json").is_file() {
        "npm install".into()
    } else {
        "true".into()
    }
}

/// One snapshot as the index remembers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRecord {
    /// Snapshot name.
    pub name: String,
    /// Fingerprint it was built for.
    pub fingerprint: String,
    /// Project commit it holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// boxd version.
    pub version: u64,
    /// When it was built, Unix milliseconds.
    pub built_ms: u64,
}

/// The snapshots built, by key ([`BASE_SNAPSHOT`] or a project id),
/// kept in `AURA_HOME/cloud/snapshots.json`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotIndex {
    /// Records by key.
    pub records: BTreeMap<String, SnapshotRecord>,
}

impl SnapshotIndex {
    /// Read the index; a missing or unreadable file is an empty index.
    pub async fn load(path: &Path) -> Self {
        match tokio::fs::read(path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
                tracing::warn!(path = %path.display(), error = %err, "snapshot index unreadable; starting empty");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Write the index atomically (temp file, then rename).
    ///
    /// # Errors
    ///
    /// When the file can't be written.
    pub async fn save(&self, path: &Path) -> CloudResult<()> {
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir)
                .await
                .map_err(|err| CloudError::io(format!("create {}", dir.display()), err))?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|err| CloudError::InvalidResponse {
            op: "snapshot index",
            message: err.to_string(),
        })?;
        let tmp = path.with_extension("json.tmp");
        tokio::fs::write(&tmp, bytes)
            .await
            .map_err(|err| CloudError::io(format!("write {}", tmp.display()), err))?;
        tokio::fs::rename(&tmp, path)
            .await
            .map_err(|err| CloudError::io(format!("replace {}", path.display()), err))
    }
}

/// The aura binary the base snapshot gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuraBinary {
    /// Upload this file (a Linux build matching the machines' arch).
    Upload(PathBuf),
    /// The base packages already install it (tests).
    Installed,
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
    index_path: PathBuf,
    /// One build at a time per process; also guards the index file.
    index: Mutex<Option<SnapshotIndex>>,
}

impl SnapshotBuilder {
    /// A builder; the index lives at `index_path`.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        provider: Arc<dyn VmProvider>,
        ledger: Arc<dyn VmLedger>,
        install: impl Into<String>,
        size: MachineSize,
        isolated: bool,
        aura_version: impl Into<String>,
        binary: AuraBinary,
        index_path: PathBuf,
    ) -> Self {
        Self {
            provider,
            ledger,
            install: install.into(),
            size,
            isolated,
            aura_version: aura_version.into(),
            binary,
            index_path,
            index: Mutex::new(None),
        }
    }

    async fn record(&self, key: &str) -> Option<SnapshotRecord> {
        let mut guard = self.index.lock().await;
        if guard.is_none() {
            *guard = Some(SnapshotIndex::load(&self.index_path).await);
        }
        guard
            .as_ref()
            .and_then(|index| index.records.get(key).cloned())
    }

    async fn store(&self, key: &str, record: SnapshotRecord) -> CloudResult<()> {
        let mut guard = self.index.lock().await;
        let index = guard.get_or_insert_with(SnapshotIndex::default);
        index.records.insert(key.to_string(), record);
        index.save(&self.index_path).await
    }

    /// Whether `record` is current for `fingerprint` and still on boxd.
    async fn usable(
        &self,
        record: Option<&SnapshotRecord>,
        fingerprint: &str,
    ) -> CloudResult<bool> {
        let Some(record) = record.filter(|r| r.fingerprint == fingerprint) else {
            return Ok(false);
        };
        Ok(self
            .provider
            .snapshot(&record.name)
            .await?
            .is_some_and(|snapshot| snapshot.is_ready()))
    }

    /// The base snapshot, built when missing or stale.
    ///
    /// # Errors
    ///
    /// When a build step fails.
    pub async fn ensure_base(&self) -> CloudResult<SnapshotRecord> {
        let fingerprint = base_fingerprint(self.size, &self.aura_version);
        let current = self.record(BASE_SNAPSHOT).await;
        if self.usable(current.as_ref(), &fingerprint).await? {
            if let Some(record) = current {
                return Ok(record);
            }
        }
        let mut steps = Vec::new();
        if let AuraBinary::Upload(path) = &self.binary {
            let bytes = tokio::fs::read(path).await.map_err(|err| {
                CloudError::io(format!("read aura binary {}", path.display()), err)
            })?;
            steps.push(Step::Upload {
                path: "/tmp/aura".into(),
                bytes,
            });
            steps.push(Step::Run {
                command: "sudo install -m 0755 /tmp/aura /usr/local/bin/aura && rm -f /tmp/aura"
                    .into(),
                timeout: SHORT_STEP_TIMEOUT,
            });
        }
        let origin = SnapshotOrigin {
            name: BASE_SNAPSHOT.into(),
            fingerprint: fingerprint.clone(),
            commit: None,
            built_ms: now_ms(),
        };
        let ledger = names::project_ledger("base");
        let version = self
            .build(
                BASE_SNAPSHOT,
                MachineOrigin::Fresh,
                base_packages(),
                steps,
                &origin,
                None,
                &ledger,
            )
            .await?;
        let record = SnapshotRecord {
            name: BASE_SNAPSHOT.into(),
            fingerprint,
            commit: None,
            version,
            built_ms: origin.built_ms,
        };
        self.store(BASE_SNAPSHOT, record.clone()).await?;
        Ok(record)
    }

    /// The project's snapshot, built when missing or stale. `force`
    /// rebuilds it anyway (the delta grew too large).
    ///
    /// # Errors
    ///
    /// When the project is not a git repository with a commit, or a
    /// build step fails.
    pub async fn ensure_project(
        &self,
        project_id: &str,
        project_dir: &Path,
        build_command: Option<&str>,
        force: bool,
    ) -> CloudResult<SnapshotRecord> {
        let base = self.ensure_base().await?;
        let dir = project_dir.to_path_buf();
        let command = build_command.map_or_else(|| default_build_command(&dir), str::to_string);
        let fingerprint = {
            let dir = dir.clone();
            let base = base.fingerprint.clone();
            let command = command.clone();
            tokio::task::spawn_blocking(move || project_fingerprint(&dir, &base, &command))
                .await
                .map_err(|err| CloudError::io("fingerprint project", std::io::Error::other(err)))?
        };
        let current = self.record(project_id).await;
        if !force && self.usable(current.as_ref(), &fingerprint).await? {
            if let Some(record) = current {
                return Ok(record);
            }
        }
        let (commit, archive) = {
            let dir = dir.clone();
            tokio::task::spawn_blocking(move || {
                let commit = delta::head_commit(&dir)?;
                let archive = delta::archive_commit(&dir, &commit)?;
                Ok::<_, CloudError>((commit, archive))
            })
            .await
            .map_err(|err| CloudError::io("archive project", std::io::Error::other(err)))??
        };
        let name = names::project_snapshot(project_id);
        let steps = vec![
            Step::Upload {
                path: "/tmp/aura-project.tgz".into(),
                bytes: archive,
            },
            Step::Run {
                command: delta::APPLY_ARCHIVE.into(),
                timeout: SHORT_STEP_TIMEOUT,
            },
            Step::Run {
                command: format!("set -e; cd \"$AURA_WORKSPACE\"; {command}"),
                timeout: STEP_TIMEOUT,
            },
        ];
        let origin = SnapshotOrigin {
            name: name.clone(),
            fingerprint: fingerprint.clone(),
            commit: Some(commit.clone()),
            built_ms: now_ms(),
        };
        let ledger = names::project_ledger(project_id);
        let version = self
            .build(
                &name,
                MachineOrigin::Snapshot(base.name.clone()),
                Vec::new(),
                steps,
                &origin,
                Some(project_id),
                &ledger,
            )
            .await?;
        let record = SnapshotRecord {
            name,
            fingerprint,
            commit: Some(commit),
            version,
            built_ms: origin.built_ms,
        };
        self.store(project_id, record.clone()).await?;
        Ok(record)
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

    #[allow(clippy::too_many_arguments)]
    async fn build(
        &self,
        snapshot: &str,
        origin: MachineOrigin,
        packages: Vec<String>,
        steps: Vec<Step>,
        manifest: &SnapshotOrigin,
        project: Option<&str>,
        ledger: &str,
    ) -> CloudResult<u64> {
        let nonce = &uuid::Uuid::new_v4().simple().to_string()[..6];
        let mut labels = BTreeMap::new();
        labels.insert(LABEL_INSTALL.to_string(), self.install.clone());
        labels.insert(LABEL_ROLE.to_string(), ROLE_BUILDER.to_string());
        if let Some(project) = project {
            labels.insert(LABEL_PROJECT.to_string(), project.to_string());
        }
        let spec = NewMachine {
            name: names::builder_name(snapshot, nonce),
            size: self.size,
            isolated: self.isolated,
            labels,
            env: MachineEnv::for_agent(self.size.vcpu()).into_map(),
            // boxd runs first-boot packages as root in the background;
            // the setup runs as steps instead, in order, as the user.
            packages: Vec::new(),
            networks: Vec::new(),
            delete_after_secs: Some(BUILDER_TTL_SECS),
        };
        tracing::info!(snapshot, builder = %spec.name, "building cloud snapshot");
        let machine = self.provider.create(&spec, &origin).await?;
        let mut all = packages
            .into_iter()
            .map(|command| Step::Run {
                command,
                timeout: STEP_TIMEOUT,
            })
            .collect::<Vec<_>>();
        all.extend(steps);
        let outcome = self.run_build(&machine.name, all, manifest, snapshot).await;
        if let Err(err) = self.provider.destroy(&machine.name).await {
            tracing::warn!(builder = %machine.name, error = %err, "could not destroy a snapshot builder; its delete-after will");
        }
        match outcome {
            Ok(version) => {
                self.ledger.record(
                    ledger,
                    &VmPayload {
                        snapshot: Some(snapshot.to_string()),
                        reason: manifest.commit.clone(),
                        ..VmPayload::new(VmEvent::SnapshotSaved, machine.id, machine.name)
                    },
                );
                Ok(version)
            }
            Err(err) => {
                self.ledger.record(
                    ledger,
                    &VmPayload {
                        snapshot: Some(snapshot.to_string()),
                        reason: Some(err.to_string()),
                        ..VmPayload::new(VmEvent::Error, machine.id, machine.name)
                    },
                );
                Err(err)
            }
        }
    }

    async fn run_build(
        &self,
        builder: &str,
        steps: Vec<Step>,
        manifest: &SnapshotOrigin,
        snapshot: &str,
    ) -> CloudResult<u64> {
        let env = MachineEnv::for_agent(self.size.vcpu());
        let manifest_json =
            serde_json::to_vec_pretty(manifest).map_err(|err| CloudError::InvalidResponse {
                op: "snapshot manifest",
                message: err.to_string(),
            })?;
        let mut all = vec![
            // Wait for the image's own first boot, then lay down the
            // machine env.
            Step::Run {
                command: "command -v cloud-init >/dev/null 2>&1 && cloud-init status --wait >/dev/null 2>&1; true".into(),
                timeout: STEP_TIMEOUT,
            },
            Step::Upload {
                path: "/tmp/machine.env".into(),
                bytes: env.as_exports().replace("export ", "").into_bytes(),
            },
            Step::Run {
                command: "sudo mkdir -p /etc/aura && sudo mv /tmp/machine.env /etc/aura/machine.env".into(),
                timeout: SHORT_STEP_TIMEOUT,
            },
        ];
        all.extend(steps);
        all.push(Step::Upload {
            path: "/tmp/snapshot.json".into(),
            bytes: manifest_json,
        });
        all.push(Step::Run {
            command: format!("sudo mv /tmp/snapshot.json {SNAPSHOT_MANIFEST}"),
            timeout: SHORT_STEP_TIMEOUT,
        });
        all.push(Step::Run {
            command: format!("{STOP_NODE}; {START_NODE}"),
            timeout: SHORT_STEP_TIMEOUT,
        });
        for step in all {
            self.run_step(builder, step).await?;
        }
        let saved = self.provider.save_snapshot(builder, snapshot).await?;
        let deadline = tokio::time::Instant::now() + READY_WAIT;
        loop {
            if self
                .provider
                .snapshot(snapshot)
                .await?
                .is_some_and(|s| s.is_ready() && s.version >= saved.version)
            {
                return Ok(saved.version);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(CloudError::Timeout {
                    op: "CreateSnapshot",
                    after: READY_WAIT,
                });
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn run_step(&self, builder: &str, step: Step) -> CloudResult<()> {
        match step {
            Step::Upload { path, bytes } => {
                self.provider.upload(builder, &path, bytes).await?;
                Ok(())
            }
            Step::Run { command, timeout } => {
                let wrapped = format!("set -a; [ -f /etc/aura/machine.env ] && . /etc/aura/machine.env; set +a; {command}");
                let output = self.provider.exec(builder, &wrapped, None, timeout).await?;
                if output.success() {
                    Ok(())
                } else {
                    Err(CloudError::ExecFailed {
                        machine: builder.to_string(),
                        command,
                        exit_code: output.exit_code,
                        stderr: output.stderr_tail(800),
                    })
                }
            }
        }
    }
}

enum Step {
    Upload { path: String, bytes: Vec<u8> },
    Run { command: String, timeout: Duration },
}

fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeProvider;
    use crate::fleet::MemoryLedger;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        assert!(Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .expect("git")
            .success());
    }

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "t@t"]);
        git(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname='x'\n").expect("write");
        std::fs::write(dir.path().join("Cargo.lock"), "v1").expect("write");
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        dir
    }

    fn builder(fake: &FakeProvider, home: &Path) -> (SnapshotBuilder, Arc<MemoryLedger>) {
        let ledger = Arc::new(MemoryLedger::default());
        let builder = SnapshotBuilder::new(
            Arc::new(fake.clone()),
            ledger.clone(),
            "inst-1",
            MachineSize::S4x16,
            true,
            "0.1.0",
            AuraBinary::Installed,
            home.join("cloud/snapshots.json"),
        );
        (builder, ledger)
    }

    #[test]
    fn fingerprints_follow_lockfiles_and_the_build_command() {
        let dir = project();
        let a = project_fingerprint(dir.path(), "base1", "cargo build");
        assert_eq!(a, project_fingerprint(dir.path(), "base1", "cargo build"));
        assert_ne!(a, project_fingerprint(dir.path(), "base1", "cargo test"));
        assert_ne!(a, project_fingerprint(dir.path(), "base2", "cargo build"));
        std::fs::write(dir.path().join("Cargo.lock"), "v2").expect("write");
        assert_ne!(a, project_fingerprint(dir.path(), "base1", "cargo build"));
        assert_ne!(
            base_fingerprint(MachineSize::S4x16, "0.1.0"),
            base_fingerprint(MachineSize::S8x32, "0.1.0")
        );
        assert_eq!(
            default_build_command(dir.path()),
            "cargo fetch && cargo test --workspace --no-run"
        );
    }

    #[tokio::test]
    async fn a_project_snapshot_is_built_once_and_reused() {
        let dir = project();
        let home = tempfile::tempdir().expect("home");
        let fake = FakeProvider::new();
        let (builder, ledger) = builder(&fake, home.path());

        let first = builder
            .ensure_project("p1", dir.path(), None, false)
            .await
            .expect("build");
        assert_eq!(first.name, "aura-proj-p1");
        assert!(first.commit.is_some());
        // Builders are gone, the base and project snapshots exist.
        assert!(
            fake.machine_names().is_empty(),
            "{:?}",
            fake.machine_names()
        );
        assert_eq!(fake.calls_matching("SaveSnapshot(").len(), 2);
        assert!(fake.calls().iter().any(|c| c.contains("cargo fetch")));

        let again = builder
            .ensure_project("p1", dir.path(), None, false)
            .await
            .expect("reuse");
        assert_eq!(again, first);
        assert_eq!(
            fake.calls_matching("SaveSnapshot(").len(),
            2,
            "nothing rebuilt"
        );

        std::fs::write(dir.path().join("Cargo.lock"), "v2").expect("write");
        builder
            .ensure_project("p1", dir.path(), None, false)
            .await
            .expect("rebuild");
        assert_eq!(
            fake.calls_matching("SaveSnapshot(").len(),
            3,
            "only the project rebuilt"
        );
        assert_eq!(
            ledger.events("0://cloud/project/p1"),
            vec![VmEvent::SnapshotSaved, VmEvent::SnapshotSaved]
        );

        // The index survives a restart.
        let (restarted, _) = super::tests::builder(&fake, home.path());
        let record = restarted
            .ensure_project("p1", dir.path(), None, false)
            .await
            .expect("reuse");
        assert_eq!(fake.calls_matching("SaveSnapshot(").len(), 3);
        assert_eq!(record.version, 2);
    }

    #[tokio::test]
    async fn a_failed_step_destroys_the_builder_and_records_the_error() {
        let dir = project();
        let home = tempfile::tempdir().expect("home");
        let fake = FakeProvider::new();
        fake.on_exec(Arc::new(|_, command, _| crate::ExecOutput {
            exit_code: i32::from(command.contains("cargo fetch")),
            stdout: Vec::new(),
            stderr: b"error: failed to fetch".to_vec(),
        }));
        let (builder, ledger) = builder(&fake, home.path());
        let err = builder
            .ensure_project("p1", dir.path(), None, false)
            .await
            .expect_err("build fails");
        assert!(err.to_string().contains("failed to fetch"), "{err}");
        assert!(fake.machine_names().is_empty());
        assert_eq!(ledger.events("0://cloud/project/p1"), vec![VmEvent::Error]);
    }

    #[tokio::test]
    async fn builders_delete_themselves_eventually() {
        let dir = project();
        let home = tempfile::tempdir().expect("home");
        let fake = FakeProvider::new();
        let (builder, _) = builder(&fake, home.path());
        builder
            .ensure_project("p1", dir.path(), None, false)
            .await
            .expect("build");
        let creates = fake.calls_matching("Create(");
        assert_eq!(creates.len(), 2, "{creates:?}");
        for create in &creates {
            assert!(create.contains("-build-"), "{create}");
            assert!(
                create.ends_with(&format!("ttl=Some({BUILDER_TTL_SECS}))")),
                "{create}"
            );
        }
        assert!(
            creates[1].contains("Snapshot(\"aura-base\")"),
            "{}",
            creates[1]
        );
    }
}
