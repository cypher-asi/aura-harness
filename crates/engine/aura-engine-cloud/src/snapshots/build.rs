//! One snapshot build: a builder machine is made, set up step by step,
//! saved as the snapshot once its node runs, and destroyed.

use std::collections::BTreeMap;
use std::time::Duration;

use aura_core_protocol::cloud::SnapshotOrigin;
use aura_core_types::{VmEvent, VmPayload};

use super::{
    SnapshotBuilder, BUILDER_TTL_SECS, CHECK_BINARY, READY_WAIT, SHORT_STEP_TIMEOUT,
    SNAPSHOT_MANIFEST, START_NODE, STEP_TIMEOUT, STOP_NODE,
};
use crate::boot::{human_bytes, BootReporter, BootStage};
use crate::boxd::CREATE_TIMEOUT;
use crate::error::{CloudError, CloudResult};
use crate::machine_env::MachineEnv;
use crate::names::{self, LABEL_INSTALL, LABEL_PROJECT, LABEL_ROLE, ROLE_BUILDER};
use crate::provider::{MachineInfo, MachineOrigin, NewMachine};

/// Longest command shown on a waiting line, in characters.
const MAX_WAIT_COMMAND_CHARS: usize = 80;

pub(super) enum Step {
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

/// One snapshot to build: what its builder is made from, what runs on
/// it, and where the outcome is recorded.
pub(super) struct Build<'a> {
    /// The snapshot to save.
    pub(super) snapshot: &'a str,
    /// What the builder machine is made from.
    pub(super) origin: MachineOrigin,
    /// Setup commands, run before `steps`.
    pub(super) packages: Vec<String>,
    pub(super) steps: Vec<Step>,
    /// What the snapshot says it holds.
    pub(super) manifest: &'a SnapshotOrigin,
    /// The project it is for, labelled on the builder.
    pub(super) project: Option<&'a str>,
    /// The ledger agent the outcome is recorded on.
    pub(super) ledger: &'a str,
}

impl SnapshotBuilder {
    /// Make a builder, run `build`'s packages and steps on it, save it as
    /// the snapshot and destroy it, recording the outcome on the build's
    /// ledger. Returns the snapshot's version.
    pub(super) async fn build(&self, build: Build<'_>, boot: &BootReporter) -> CloudResult<u64> {
        let Build {
            snapshot,
            origin,
            packages,
            steps,
            manifest,
            project,
            ledger,
        } = build;
        let spec = self.builder_spec(snapshot, project);
        let machine = self.create_builder(&spec, &origin, snapshot, boot).await?;
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
        let (event, reason) = match &outcome {
            Ok(_) => (VmEvent::SnapshotSaved, manifest.commit.clone()),
            Err(err) => (VmEvent::Error, Some(err.to_string())),
        };
        self.ledger.record(
            ledger,
            &VmPayload {
                snapshot: Some(snapshot.to_string()),
                reason,
                ..VmPayload::new(event, machine.id, machine.name)
            },
        );
        outcome
    }

    /// The builder machine for `snapshot`, labelled as this install's
    /// builder (for `project`), deleting itself after
    /// [`BUILDER_TTL_SECS`].
    fn builder_spec(&self, snapshot: &str, project: Option<&str>) -> NewMachine {
        let nonce = &uuid::Uuid::new_v4().simple().to_string()[..6];
        let mut labels = BTreeMap::new();
        labels.insert(LABEL_INSTALL.to_string(), self.install.clone());
        labels.insert(LABEL_ROLE.to_string(), ROLE_BUILDER.to_string());
        if let Some(project) = project {
            labels.insert(LABEL_PROJECT.to_string(), project.to_string());
        }
        NewMachine {
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
        }
    }

    /// Create the builder `spec` from `origin`, showing it on `boot`.
    async fn create_builder(
        &self,
        spec: &NewMachine,
        origin: &MachineOrigin,
        snapshot: &str,
        boot: &BootReporter,
    ) -> CloudResult<MachineInfo> {
        tracing::info!(snapshot, builder = %spec.name, "building cloud snapshot");
        boot.log(
            BootStage::Snapshot,
            false,
            format!("creating builder machine {}", spec.name),
        );
        let machine = boot
            .wait(
                BootStage::Snapshot,
                format!("boxd is creating builder machine {}", spec.name),
                Some(CREATE_TIMEOUT),
                self.provider.create(spec, origin),
            )
            .await?;
        boot.fact(
            "builder",
            format!(
                "{} ({} vCPU, {})",
                machine.name,
                machine.vcpu,
                human_bytes(machine.memory_bytes)
            ),
        );
        Ok(machine)
    }

    async fn run_build(
        &self,
        builder: &str,
        steps: Vec<Step>,
        manifest: &SnapshotOrigin,
        snapshot: &str,
        boot: &BootReporter,
    ) -> CloudResult<u64> {
        let all = self.framed(steps, manifest)?;
        let total = all.len() as u64;
        for (done, step) in (0..).zip(all) {
            boot.progress(BootStage::Snapshot, done, total);
            self.run_step(builder, step, boot).await?;
        }
        boot.progress(BootStage::Snapshot, total, total);
        self.save(builder, snapshot, boot).await
    }

    /// `steps` between what every builder runs: the image's first boot
    /// and the machine env before them, the snapshot manifest and a
    /// fresh node after.
    fn framed(&self, steps: Vec<Step>, manifest: &SnapshotOrigin) -> CloudResult<Vec<Step>> {
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
        Ok(all)
    }

    /// Save `builder` as `snapshot` and wait until boxd has it ready.
    /// Returns its version.
    async fn save(&self, builder: &str, snapshot: &str, boot: &BootReporter) -> CloudResult<u64> {
        boot.log(
            BootStage::Snapshot,
            false,
            format!("saving snapshot {snapshot}"),
        );
        let saved = boot
            .wait(
                BootStage::Snapshot,
                format!("boxd is saving {builder} as snapshot {snapshot}"),
                Some(CREATE_TIMEOUT),
                self.provider.save_snapshot(builder, snapshot),
            )
            .await?;
        let deadline = tokio::time::Instant::now() + READY_WAIT;
        let ready = format!(
            "boxd is getting snapshot {snapshot} v{} ready",
            saved.version
        );
        // One wait for the whole poll, its line updated with what boxd
        // last said; an update that doesn't clear it keeps its clock.
        let poll = self.poll_ready(snapshot, saved.version, deadline, &ready, boot);
        boot.wait(BootStage::Snapshot, ready.clone(), Some(READY_WAIT), poll)
            .await
    }

    /// Ask boxd about `snapshot` until it is ready at `version` or later,
    /// or `deadline` passes, adding what boxd says to the `ready` line.
    async fn poll_ready(
        &self,
        snapshot: &str,
        version: u64,
        deadline: tokio::time::Instant,
        ready: &str,
        boot: &BootReporter,
    ) -> CloudResult<u64> {
        loop {
            let found = self.provider.snapshot(snapshot).await?;
            if found
                .as_ref()
                .is_some_and(|s| s.is_ready() && s.version >= version)
            {
                return Ok(version);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(CloudError::Timeout {
                    op: "CreateSnapshot",
                    after: READY_WAIT,
                });
            }
            let status = found.map_or_else(
                || "not listed yet".to_string(),
                |s| format!("{} at v{}", s.status, s.version),
            );
            boot.waiting(
                BootStage::Snapshot,
                format!("{ready} (boxd says {status})"),
                Some(READY_WAIT),
            );
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn run_step(&self, builder: &str, step: Step, boot: &BootReporter) -> CloudResult<()> {
        match step {
            Step::Upload { path, bytes } => self.upload_step(builder, &path, bytes, boot).await,
            Step::Run { command, timeout } => {
                self.run_command(builder, command, timeout, boot).await
            }
            Step::CheckBinary => self.check_binary(builder, boot).await,
        }
    }

    async fn upload_step(
        &self,
        builder: &str,
        path: &str,
        bytes: Vec<u8>,
        boot: &BootReporter,
    ) -> CloudResult<()> {
        let size = human_bytes(bytes.len() as u64);
        boot.log(
            BootStage::Snapshot,
            false,
            format!("upload {path} ({size})"),
        );
        boot.wait(
            BootStage::Snapshot,
            format!("uploading {path} ({size}) to {builder}"),
            Some(CREATE_TIMEOUT),
            self.provider
                .upload_reported(builder, path, bytes, &mut |sent, total| {
                    boot.waiting(
                        BootStage::Snapshot,
                        uploading(path, sent, total, builder),
                        Some(CREATE_TIMEOUT),
                    );
                }),
        )
        .await?;
        Ok(())
    }

    async fn run_command(
        &self,
        builder: &str,
        command: String,
        timeout: Duration,
        boot: &BootReporter,
    ) -> CloudResult<()> {
        boot.log(BootStage::Snapshot, false, format!("$ {command}"));
        let wrapped = format!(
            "set -a; [ -f /etc/aura/machine.env ] && . /etc/aura/machine.env; set +a; {command}"
        );
        let mut lines = boot.output(BootStage::Snapshot);
        let output = boot
            .wait(
                BootStage::Snapshot,
                step_waits_on(&command, builder),
                Some(timeout),
                self.provider.exec_streaming(
                    builder,
                    &wrapped,
                    None,
                    timeout,
                    &mut |stderr, bytes| {
                        lines.chunk(stderr, bytes);
                    },
                ),
            )
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

    async fn check_binary(&self, builder: &str, boot: &BootReporter) -> CloudResult<()> {
        let output = boot
            .wait(
                BootStage::Snapshot,
                format!("checking aura starts on {builder}"),
                Some(SHORT_STEP_TIMEOUT),
                self.provider
                    .exec(builder, CHECK_BINARY, None, SHORT_STEP_TIMEOUT),
            )
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

/// An upload's waiting line: `uploading /tmp/aura (40 MiB of 140 MiB) to b1`.
#[must_use]
pub fn uploading(path: &str, sent: u64, total: u64, machine: &str) -> String {
    if sent >= total {
        return format!(
            "uploading {path} ({}) to {machine}: sent, waiting for boxd to write it",
            human_bytes(total)
        );
    }
    format!(
        "uploading {path} ({} of {}) to {machine}",
        human_bytes(sent),
        human_bytes(total)
    )
}

/// What a build step waits on, as shown: the image's first boot by name
/// (its output goes nowhere, so this line is all a boot shows of it),
/// any other command cut short.
pub(super) fn step_waits_on(command: &str, builder: &str) -> String {
    if command.contains("cloud-init status --wait") {
        return format!("waiting for {builder}'s first boot (cloud-init)");
    }
    let one_line = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let shown = if one_line.chars().count() > MAX_WAIT_COMMAND_CHARS {
        let mut cut: String = one_line.chars().take(MAX_WAIT_COMMAND_CHARS - 1).collect();
        cut.push('…');
        cut
    } else {
        one_line
    };
    format!("`{shown}` on {builder}")
}
