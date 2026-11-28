//! Keeping the base and each project's snapshot current: reused while
//! the fingerprint holds and boxd still has it, built otherwise.

use std::path::Path;

use aura_core_protocol::cloud::SnapshotOrigin;
use aura_core_types::time::now_ms;

use super::binary::LocalBinary;
use super::build::{Build, Step};
use super::{
    base_fingerprint, base_packages, default_build_command, project_fingerprint, AuraBinary,
    SnapshotBuilder, SnapshotRecord, SHORT_STEP_TIMEOUT, STEP_TIMEOUT,
};
use crate::boot::{human_bytes, BootReporter, BootStage};
use crate::boxd::CALL_TIMEOUT;
use crate::delta::{self, HistoryBundle};
use crate::error::{CloudError, CloudResult};
use crate::exec_errors::is_transient;
use crate::names::{self, BASE_SNAPSHOT};
use crate::provider::MachineOrigin;

/// What a project snapshot is built from.
#[derive(Clone)]
struct ProjectSource {
    /// The base snapshot the builder is made from.
    base: String,
    /// The fingerprint it is built for.
    fingerprint: String,
    /// The project's history at HEAD.
    history: HistoryBundle,
    /// The build run on it.
    command: String,
}

impl SnapshotBuilder {
    /// Whether `record` is current for `fingerprint` and still on boxd.
    async fn usable(
        &self,
        record: Option<&SnapshotRecord>,
        fingerprint: &str,
        boot: &BootReporter,
    ) -> CloudResult<bool> {
        let Some(record) = record.filter(|r| r.fingerprint == fingerprint) else {
            return Ok(false);
        };
        Ok(boot
            .wait(
                BootStage::Snapshot,
                format!("boxd is looking up snapshot {}", record.name),
                Some(CALL_TIMEOUT),
                self.provider.snapshot(&record.name),
            )
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
        let gate = self.gate(BASE_SNAPSHOT);
        let _building = wait_for(&gate, BASE_SNAPSHOT, boot).await;
        let local = self.local_binary().await?;
        // The binary itself is part of the base: a new build of the same
        // version must still reach new machines.
        let version = local.as_ref().map_or_else(
            || self.aura_version.clone(),
            |local| format!("{}+{}", self.aura_version, local.digest),
        );
        let fingerprint = base_fingerprint(self.size, &version);
        let current = self.record(BASE_SNAPSHOT).await;
        if self.usable(current.as_ref(), &fingerprint, boot).await? {
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
        let steps = base_steps(local, &self.home)?;
        self.build_base(fingerprint, steps, boot).await
    }

    /// The binary to upload, read and inspected off the async runtime;
    /// `None` when the base packages install it.
    pub(crate) async fn local_binary(&self) -> CloudResult<Option<LocalBinary>> {
        match &self.binary {
            AuraBinary::Upload(path) => {
                let path = path.clone();
                let local = tokio::task::spawn_blocking(move || LocalBinary::read(path))
                    .await
                    .map_err(|err| {
                        CloudError::io("inspect aura binary", std::io::Error::other(err))
                    })??;
                Ok(Some(local))
            }
            AuraBinary::Installed => Ok(None),
        }
    }

    /// Build the base snapshot for `fingerprint` from a fresh machine:
    /// its packages, then `steps`. Remembers and returns it.
    async fn build_base(
        &self,
        fingerprint: String,
        steps: Vec<Step>,
        boot: &BootReporter,
    ) -> CloudResult<SnapshotRecord> {
        let origin = SnapshotOrigin {
            name: BASE_SNAPSHOT.into(),
            fingerprint: fingerprint.clone(),
            commit: None,
            built_ms: now_ms(),
        };
        let ledger = names::project_ledger("base");
        let build = Build {
            snapshot: BASE_SNAPSHOT,
            origin: MachineOrigin::Fresh,
            packages: base_packages(),
            steps,
            manifest: &origin,
            project: None,
            ledger: &ledger,
        };
        let version = self.build(build, boot).await?;
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
        let gate = self.gate(project_id);
        let _building = wait_for(&gate, &names::project_snapshot(project_id), boot).await;
        let command =
            build_command.map_or_else(|| default_build_command(project_dir), str::to_string);
        let fingerprint = fingerprint_project(project_dir, &base.fingerprint, &command).await?;
        let current = self.record(project_id).await;
        if !force && self.usable(current.as_ref(), &fingerprint, boot).await? {
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
        let history = bundle_history(project_dir, boot).await?;
        let source = ProjectSource {
            base: base.name,
            fingerprint,
            history,
            command,
        };
        self.build_project_retried(project_id, source, boot).await
    }

    /// [`Self::build_project`], once more on a fresh builder when the
    /// first build failed for the machine's sake, not the code's
    /// ([`is_transient`]): killed for memory, out of disk, a download.
    async fn build_project_retried(
        &self,
        project_id: &str,
        source: ProjectSource,
        boot: &BootReporter,
    ) -> CloudResult<SnapshotRecord> {
        let again = source.clone();
        match self.build_project(project_id, source, boot).await {
            Err(err) if is_transient(&err) => {
                tracing::warn!(project = project_id, error = %err, "snapshot build failed for the machine; trying once more");
                boot.fact(
                    "snapshot",
                    format!("the build failed for its machine, trying once more: {err}"),
                );
                self.build_project(project_id, again, boot).await
            }
            outcome => outcome,
        }
    }

    /// Build `project_id`'s snapshot from `source`: its history cloned on
    /// a machine made from the base, then its build. Remembers and
    /// returns it.
    async fn build_project(
        &self,
        project_id: &str,
        source: ProjectSource,
        boot: &BootReporter,
    ) -> CloudResult<SnapshotRecord> {
        let ProjectSource {
            base,
            fingerprint,
            history,
            command,
        } = source;
        let clone = crate::workspace::clone_snapshot(&history.commit, history.upstream.as_deref())?;
        let commit = history.commit;
        let name = names::project_snapshot(project_id);
        let steps = project_steps(history.bytes, clone, &command);
        let origin = SnapshotOrigin {
            name: name.clone(),
            fingerprint: fingerprint.clone(),
            commit: Some(commit.clone()),
            built_ms: now_ms(),
        };
        let ledger = names::project_ledger(project_id);
        let build = Build {
            snapshot: &name,
            origin: MachineOrigin::Snapshot(base),
            packages: Vec::new(),
            steps,
            manifest: &origin,
            project: Some(project_id),
            ledger: &ledger,
        };
        let version = self.build(build, boot).await?;
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
}

/// The steps that put `local` on the base builder and check it starts
/// there; none when the base packages install aura.
fn base_steps(local: Option<LocalBinary>, home: &Path) -> CloudResult<Vec<Step>> {
    let mut steps = Vec::new();
    if let Some(local) = local {
        local.ensure_loads_on_machines(home)?;
        steps.push(Step::Upload {
            path: "/tmp/aura".into(),
            bytes: local.bytes,
        });
        steps.push(Step::Run {
            command: "sudo install -m 0755 /tmp/aura /usr/local/bin/aura && rm -f /tmp/aura".into(),
            timeout: SHORT_STEP_TIMEOUT,
        });
        steps.push(Step::CheckBinary { source: local.path });
    }
    Ok(steps)
}

/// [`project_fingerprint`], read off the async runtime.
async fn fingerprint_project(dir: &Path, base: &str, command: &str) -> CloudResult<String> {
    let dir = dir.to_path_buf();
    let base = base.to_string();
    let command = command.to_string();
    tokio::task::spawn_blocking(move || project_fingerprint(&dir, &base, &command))
        .await
        .map_err(|err| CloudError::io("fingerprint project", std::io::Error::other(err)))
}

/// The project's history at HEAD as a git bundle, made off the async
/// runtime and shown on `boot`.
async fn bundle_history(dir: &Path, boot: &BootReporter) -> CloudResult<HistoryBundle> {
    let dir = dir.to_path_buf();
    let history = boot
        .wait(
            BootStage::Snapshot,
            "bundling the project's history at HEAD",
            None,
            tokio::task::spawn_blocking(move || delta::bundle_head(&dir)),
        )
        .await
        .map_err(|err| CloudError::io("bundle project", std::io::Error::other(err)))??;
    boot.fact(
        "history",
        format!(
            "{} at {}",
            human_bytes(history.bytes.len() as u64),
            history.commit.chars().take(12).collect::<String>()
        ),
    );
    Ok(history)
}

/// The steps of a project build: the history `bundle` uploaded and
/// cloned by the `clone` script, then `command` in the workspace.
fn project_steps(bundle: Vec<u8>, clone: String, command: &str) -> Vec<Step> {
    vec![
        Step::Upload {
            path: crate::workspace::SNAPSHOT_BUNDLE.into(),
            bytes: bundle,
        },
        Step::Run {
            command: clone,
            timeout: SHORT_STEP_TIMEOUT,
        },
        Step::Run {
            command: format!("set -e; cd \"$AURA_WORKSPACE\"; {command}"),
            timeout: STEP_TIMEOUT,
        },
    ]
}

/// Hold `gate`, the one of `snapshot`, saying on `boot` when another
/// start is building it: this one then reuses what it built.
async fn wait_for<'a>(
    gate: &'a tokio::sync::Mutex<()>,
    snapshot: &str,
    boot: &BootReporter,
) -> tokio::sync::MutexGuard<'a, ()> {
    if let Ok(held) = gate.try_lock() {
        return held;
    }
    boot.fact(
        "snapshot",
        format!("{snapshot}: another agent's start is building it; waiting to reuse it"),
    );
    gate.lock().await
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
