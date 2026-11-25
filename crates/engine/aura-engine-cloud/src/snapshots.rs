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

use crate::boot::{human_bytes, BootReporter, BootStage};
use crate::delta;
use crate::error::{CloudError, CloudResult};
use crate::fleet::VmLedger;
use crate::glibc::{self, GlibcVersion, MACHINE_GLIBC};
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

/// The first 16 hex characters of `path`'s blake3 hash. Blocking.
///
/// # Errors
///
/// When the file can't be read.
pub fn binary_digest(path: &Path) -> CloudResult<String> {
    let mut file = std::fs::File::open(path)
        .map_err(|err| CloudError::io(format!("open {}", path.display()), err))?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|err| CloudError::io(format!("read {}", path.display()), err))?;
    Ok(hasher.finalize().to_hex()[..16].to_string())
}

/// Where the hub looks for a Linux build of aura made for the machines
/// (`scripts/build-cloud-binary.sh` installs there), under the cloud
/// home (`AURA_HOME/cloud`).
#[must_use]
pub fn cloud_binary_path(home: &Path) -> PathBuf {
    home.join("bin").join("aura")
}

/// Run on a builder right after the binary is installed: fails, with
/// the loader's complaint on stderr, when the binary can't start there.
/// `--help` is handled by argument parsing and exits 0 without touching
/// the machine.
pub const CHECK_BINARY: &str = "/usr/local/bin/aura --help >/dev/null";

/// The aura binary to upload, read once for its digest, its glibc needs
/// and the upload itself.
struct LocalBinary {
    path: PathBuf,
    bytes: Vec<u8>,
    digest: String,
    glibc: Option<GlibcVersion>,
}

impl LocalBinary {
    /// Read and inspect `path`. Blocking.
    fn read(path: PathBuf) -> CloudResult<Self> {
        let bytes = std::fs::read(&path)
            .map_err(|err| CloudError::io(format!("read aura binary {}", path.display()), err))?;
        let digest = blake3::hash(&bytes).to_hex()[..16].to_string();
        let glibc = glibc::required_glibc(&bytes);
        Ok(Self {
            path,
            bytes,
            digest,
            glibc,
        })
    }

    /// [`CloudError::IncompatibleBinary`] when the machines' glibc is
    /// too old for it.
    fn ensure_loads_on_machines(&self, home: &Path) -> CloudResult<()> {
        match self.glibc {
            Some(needs) if needs > MACHINE_GLIBC => Err(CloudError::IncompatibleBinary {
                path: self.path.clone(),
                needs,
                has: MACHINE_GLIBC,
                install_to: cloud_binary_path(home),
            }),
            needs => {
                tracing::debug!(
                    path = %self.path.display(),
                    needs = ?needs.map(|version| version.to_string()),
                    has = %MACHINE_GLIBC,
                    "aura binary's glibc fits the cloud image"
                );
                Ok(())
            }
        }
    }
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
    /// The cloud home (`AURA_HOME/cloud`).
    home: PathBuf,
    index_path: PathBuf,
    /// One build at a time per process; also guards the index file.
    index: Mutex<Option<SnapshotIndex>>,
}

impl SnapshotBuilder {
    /// A builder keeping its index in `home` (`AURA_HOME/cloud`), where
    /// [`cloud_binary_path`] also lives.
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
        home: PathBuf,
    ) -> Self {
        Self {
            provider,
            ledger,
            install: install.into(),
            size,
            isolated,
            aura_version: aura_version.into(),
            binary,
            index_path: home.join("snapshots.json"),
            home,
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
    /// [`CloudError::IncompatibleBinary`] before any machine is made when
    /// the binary needs a newer glibc than the machines have;
    /// [`CloudError::BinaryDoesNotRun`] when it fails to start on the
    /// builder anyway; otherwise when a build step fails.
    pub async fn ensure_base(&self) -> CloudResult<SnapshotRecord> {
        self.ensure_base_reported(&BootReporter::none()).await
    }

    /// [`Self::ensure_base`], showing a build on `boot`.
    ///
    /// # Errors
    ///
    /// As [`Self::ensure_base`].
    pub async fn ensure_base_reported(&self, boot: &BootReporter) -> CloudResult<SnapshotRecord> {
        let local = match &self.binary {
            AuraBinary::Upload(path) => {
                let path = path.clone();
                let local = tokio::task::spawn_blocking(move || LocalBinary::read(path))
                    .await
                    .map_err(|err| {
                        CloudError::io("inspect aura binary", std::io::Error::other(err))
                    })??;
                Some(local)
            }
            AuraBinary::Installed => None,
        };
        // The binary itself is part of the base: a new build of the same
        // version must still reach new machines.
        let version = local.as_ref().map_or_else(
            || self.aura_version.clone(),
            |local| format!("{}+{}", self.aura_version, local.digest),
        );
        let fingerprint = base_fingerprint(self.size, &version);
        let current = self.record(BASE_SNAPSHOT).await;
        if self.usable(current.as_ref(), &fingerprint).await? {
            if let Some(record) = current {
                boot.fact(
                    "base image",
                    format!("{BASE_SNAPSHOT} v{} (cached)", record.version),
                );
                return Ok(record);
            }
        }
        boot.fact(
            "base image",
            format!(
                "{BASE_SNAPSHOT}: building ({}, aura {version})",
                rebuild_reason(current.as_ref(), &fingerprint, false)
            ),
        );
        let mut steps = Vec::new();
        if let Some(local) = local {
            local.ensure_loads_on_machines(&self.home)?;
            steps.push(Step::Upload {
                path: "/tmp/aura".into(),
                bytes: local.bytes,
            });
            steps.push(Step::Run {
                command: "sudo install -m 0755 /tmp/aura /usr/local/bin/aura && rm -f /tmp/aura"
                    .into(),
                timeout: SHORT_STEP_TIMEOUT,
            });
            steps.push(Step::CheckBinary);
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
                boot,
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
        self.ensure_project_reported(
            project_id,
            project_dir,
            build_command,
            force,
            &BootReporter::none(),
        )
        .await
    }

    /// [`Self::ensure_project`], showing what it finds and builds on
    /// `boot`.
    ///
    /// # Errors
    ///
    /// When the project is not a git repository with a commit, or a
    /// build step fails.
    pub async fn ensure_project_reported(
        &self,
        project_id: &str,
        project_dir: &Path,
        build_command: Option<&str>,
        force: bool,
        boot: &BootReporter,
    ) -> CloudResult<SnapshotRecord> {
        let base = self.ensure_base_reported(boot).await?;
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
                boot.fact(
                    "snapshot",
                    format!("{} v{} (cached)", record.name, record.version),
                );
                return Ok(record);
            }
        }
        boot.fact(
            "snapshot",
            format!(
                "{}: building ({})",
                names::project_snapshot(project_id),
                rebuild_reason(current.as_ref(), &fingerprint, force)
            ),
        );
        boot.fact("build command", command.clone());
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
                boot,
            )
            .await?;
        boot.fact("snapshot", format!("{name} v{version} (built)"));
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
        boot: &BootReporter,
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
            secrets: Vec::new(),
        };
        tracing::info!(snapshot, builder = %spec.name, "building cloud snapshot");
        let machine = self.provider.create(&spec, &origin).await?;
        boot.fact(
            "builder",
            format!(
                "{} ({} vCPU, {})",
                machine.name,
                machine.vcpu,
                human_bytes(machine.memory_bytes)
            ),
        );
        let mut all = packages
            .into_iter()
            .map(|command| Step::Run {
                command,
                timeout: STEP_TIMEOUT,
            })
            .collect::<Vec<_>>();
        all.extend(steps);
        let outcome = self
            .run_build(&machine.name, all, manifest, snapshot, boot)
            .await;
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
        boot: &BootReporter,
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
        let total = all.len() as u64;
        for (done, step) in (0..).zip(all) {
            boot.progress(BootStage::Snapshot, done, total);
            self.run_step(builder, step, boot).await?;
        }
        boot.progress(BootStage::Snapshot, total, total);
        boot.log(
            BootStage::Snapshot,
            false,
            format!("saving snapshot {snapshot}"),
        );
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

    async fn run_step(&self, builder: &str, step: Step, boot: &BootReporter) -> CloudResult<()> {
        match step {
            Step::Upload { path, bytes } => {
                boot.log(
                    BootStage::Snapshot,
                    false,
                    format!("upload {path} ({})", human_bytes(bytes.len() as u64)),
                );
                self.provider.upload(builder, &path, bytes).await?;
                Ok(())
            }
            Step::Run { command, timeout } => {
                boot.log(BootStage::Snapshot, false, format!("$ {command}"));
                let wrapped = format!("set -a; [ -f /etc/aura/machine.env ] && . /etc/aura/machine.env; set +a; {command}");
                let mut lines = boot.output(BootStage::Snapshot);
                let output = self
                    .provider
                    .exec_streaming(builder, &wrapped, None, timeout, &mut |stderr, bytes| {
                        lines.chunk(stderr, bytes);
                    })
                    .await?;
                lines.finish();
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
            Step::CheckBinary => {
                let output = self
                    .provider
                    .exec(builder, CHECK_BINARY, None, SHORT_STEP_TIMEOUT)
                    .await?;
                if output.success() {
                    Ok(())
                } else {
                    Err(CloudError::BinaryDoesNotRun {
                        machine: builder.to_string(),
                        exit_code: output.exit_code,
                        stderr: output.stderr_tail(800),
                    })
                }
            }
        }
    }
}

enum Step {
    Upload {
        path: String,
        bytes: Vec<u8>,
    },
    Run {
        command: String,
        timeout: Duration,
    },
    /// [`CHECK_BINARY`], failing as [`CloudError::BinaryDoesNotRun`].
    CheckBinary,
}

/// Why a snapshot is built now rather than reused.
fn rebuild_reason(
    current: Option<&SnapshotRecord>,
    fingerprint: &str,
    force: bool,
) -> &'static str {
    match current {
        _ if force => "rebuild asked for",
        None => "first build",
        Some(record) if record.fingerprint != fingerprint => {
            "toolchain, lockfiles or build changed"
        }
        Some(_) => "no longer on boxd",
    }
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
        builder_with(fake, home, AuraBinary::Installed)
    }

    fn builder_with(
        fake: &FakeProvider,
        home: &Path,
        binary: AuraBinary,
    ) -> (SnapshotBuilder, Arc<MemoryLedger>) {
        let ledger = Arc::new(MemoryLedger::default());
        let builder = SnapshotBuilder::new(
            Arc::new(fake.clone()),
            ledger.clone(),
            "inst-1",
            MachineSize::S4x16,
            true,
            "0.1.0",
            binary,
            home.join("cloud"),
        );
        (builder, ledger)
    }

    /// A stand-in binary whose string table needs glibc `2.<minor>`.
    fn fake_binary(dir: &Path, minor: u32) -> PathBuf {
        let path = dir.join("aura");
        let strtab = format!("\x7fELF\0libc.so.6\0GLIBC_2.2.5\0GLIBC_2.{minor}\0");
        std::fs::write(&path, strtab).expect("write binary");
        path
    }

    #[tokio::test]
    async fn a_binary_needing_a_newer_glibc_is_refused_before_any_machine() {
        let home = tempfile::tempdir().expect("home");
        let binary = fake_binary(home.path(), 43);
        let fake = FakeProvider::new();
        let (builder, _) = builder_with(&fake, home.path(), AuraBinary::Upload(binary.clone()));

        let err = builder.ensure_base().await.expect_err("refused");
        assert!(
            matches!(
                &err,
                CloudError::IncompatibleBinary { needs, has, .. }
                    if *needs == GlibcVersion::new(2, 43) && *has == MACHINE_GLIBC
            ),
            "{err:?}"
        );
        let message = err.to_string();
        for part in [
            binary.display().to_string(),
            "glibc 2.43".into(),
            "have 2.39".into(),
            "scripts/build-cloud-binary.sh".into(),
            cloud_binary_path(&home.path().join("cloud"))
                .display()
                .to_string(),
            "AURA_CLOUD_BINARY".into(),
        ] {
            assert!(message.contains(&part), "{part} missing from: {message}");
        }
        assert!(
            fake.calls_matching("Create(").is_empty(),
            "{:?}",
            fake.calls()
        );
    }

    #[tokio::test]
    async fn an_uploaded_binary_is_started_once_installed() {
        let home = tempfile::tempdir().expect("home");
        let binary = fake_binary(home.path(), 34);
        let fake = FakeProvider::new();
        let (builder, _) = builder_with(&fake, home.path(), AuraBinary::Upload(binary));

        builder.ensure_base().await.expect("build");
        let calls = fake.calls();
        let position = |needle: &str| {
            calls
                .iter()
                .position(|call| call.contains(needle))
                .unwrap_or_else(|| panic!("{needle} not run: {calls:?}"))
        };
        assert!(position("install -m 0755 /tmp/aura") < position(CHECK_BINARY));
        assert!(position(CHECK_BINARY) < position("nohup aura run"));
    }

    #[tokio::test]
    async fn a_binary_that_does_not_start_on_the_builder_says_why() {
        let home = tempfile::tempdir().expect("home");
        let binary = fake_binary(home.path(), 34);
        let fake = FakeProvider::new();
        fake.on_exec(Arc::new(|_, command, _| {
            let fails = command == CHECK_BINARY;
            crate::ExecOutput {
                exit_code: i32::from(fails),
                stdout: Vec::new(),
                stderr: if fails {
                    b"aura: /lib/x86_64-linux-gnu/libm.so.6: version `GLIBC_2.43' not found"
                        .to_vec()
                } else {
                    Vec::new()
                },
            }
        }));
        let (builder, ledger) = builder_with(&fake, home.path(), AuraBinary::Upload(binary));

        let err = builder.ensure_base().await.expect_err("check fails");
        assert!(
            matches!(err, CloudError::BinaryDoesNotRun { exit_code: 1, .. }),
            "{err:?}"
        );
        let message = err.to_string();
        assert!(
            message.contains("uploaded aura binary does not run on the cloud image"),
            "{message}"
        );
        assert!(message.contains("GLIBC_2.43' not found"), "{message}");
        assert!(fake.machine_names().is_empty(), "builder destroyed");
        assert_eq!(
            ledger.events("0://cloud/project/base"),
            vec![VmEvent::Error]
        );
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
    async fn a_build_shows_its_commands_and_their_output() {
        use crate::boot::BootEvent;
        let dir = project();
        let home = tempfile::tempdir().expect("home");
        let fake = FakeProvider::new();
        fake.on_exec(Arc::new(|_, command, _| crate::ExecOutput {
            exit_code: 0,
            stdout: if command.contains("cargo fetch") {
                b"\x1b[32m   Compiling\x1b[0m x v0.1.0\n    Finished\n".to_vec()
            } else {
                Vec::new()
            },
            stderr: Vec::new(),
        }));
        let (builder, _) = builder(&fake, home.path());
        let (boot, mut rx) = BootReporter::channel();
        builder
            .ensure_project_reported("p1", dir.path(), None, false, &boot)
            .await
            .expect("build");
        let mut lines = Vec::new();
        let mut facts = Vec::new();
        let mut progressed = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                BootEvent::Log { line, .. } => lines.push(line),
                BootEvent::Fact { key, value } => facts.push((key, value)),
                BootEvent::Progress { .. } => progressed = true,
                BootEvent::Stage { .. } | BootEvent::Plan(_) => {}
            }
        }
        assert!(progressed);
        assert!(lines
            .iter()
            .any(|l| l.starts_with("$ ") && l.contains("cargo fetch")));
        assert!(
            lines.iter().any(|l| l == "   Compiling x v0.1.0"),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l == "saving snapshot aura-proj-p1"));
        assert!(facts
            .iter()
            .any(|(k, v)| k == "snapshot" && v.contains("first build")));
        assert!(facts
            .iter()
            .any(|(k, v)| k == "snapshot" && v.ends_with("(built)")));

        let (boot, mut rx) = BootReporter::channel();
        builder
            .ensure_project_reported("p1", dir.path(), None, false, &boot)
            .await
            .expect("reuse");
        let mut cached = false;
        while let Ok(event) = rx.try_recv() {
            if let BootEvent::Fact { key, value } = event {
                cached |= key == "snapshot" && value.ends_with("(cached)");
            }
        }
        assert!(cached);
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

    #[test]
    fn binary_digests_change_with_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aura");
        std::fs::write(&path, b"build one").expect("write");
        let one = binary_digest(&path).expect("digest");
        std::fs::write(&path, b"build two").expect("write");
        assert_ne!(binary_digest(&path).expect("digest"), one);
        assert_eq!(one.len(), 16);
    }
}
