//! Seeding a new machine: the project's files and commits, scripts run on
//! it, and the agent's record imported into its node.

use crate::boot::human_bytes;
use crate::fleet::MachineView;
use crate::snapshots::{self};
use crate::workspace::{self, BranchStart};
use crate::{boxd, delta, BootReporter, BootStage, CloudError, CloudResult};
use aura_core_protocol::cloud::SyncImport;
use aura_core_types::{AgentId, RecordEntry};
use std::path::Path;

use super::node_client::NODE_TIMEOUT;

use super::{short_commit, CloudHub, PackedProject, PlaceRequest, APPLY_TIMEOUT};

impl CloudHub {
    /// Bring the project files up to date on the machine and give its
    /// node the agent's record.
    pub(super) async fn seed(
        &self,
        request: &PlaceRequest,
        view: &MachineView,
        snapshot_commit: Option<&str>,
        branch: &str,
        remote_url: Option<&str>,
        boot: &BootReporter,
    ) -> CloudResult<()> {
        // The branch the project's merge queue lands on, as committed on
        // main: what the machine pulls from GitHub before the agent works.
        let target = aura_exec_merge_queue::resolve_target(&request.project_dir).await;
        boot.fact("merge target", target.clone());
        let to = Target {
            branch,
            target: &target,
            remote_url,
        };
        match snapshot_commit {
            Some(commit) => {
                boot.run(
                    BootStage::SyncFiles,
                    self.sync_files(request, view, commit, to, boot),
                )
                .await?;
            }
            None => {
                boot.run(BootStage::SyncFiles, async {
                    boot.fact("files", "the parent's, on a branch of its own");
                    let script = workspace::fork_branch(branch, &target)?;
                    self.run_script(view, BootStage::SyncFiles, "branch the fork", &script, boot)
                        .await
                        .map(|stdout| report_pull(&stdout, boot))
                })
                .await?;
            }
        }
        boot.run(
            BootStage::ImportRecord,
            self.import_record(request, view, boot),
        )
        .await
    }

    /// Bring the machine's workspace to the project here: the commits
    /// made since the snapshot's `snapshot_commit`, the agent's branch at
    /// HEAD, and the uncommitted changes on top.
    pub(super) async fn sync_files(
        &self,
        request: &PlaceRequest,
        view: &MachineView,
        snapshot_commit: &str,
        to: Target<'_>,
        boot: &BootReporter,
    ) -> CloudResult<()> {
        let Target {
            branch,
            target,
            remote_url,
        } = to;
        boot.fact("snapshot commit", short_commit(snapshot_commit));
        let dir = request.project_dir.clone();
        let base = snapshot_commit.to_string();
        let packed = boot
            .wait(
                BootStage::SyncFiles,
                format!(
                    "bundling the project's commits since {} and packing its changes",
                    short_commit(snapshot_commit)
                ),
                None,
                tokio::task::spawn_blocking(move || pack_project(&dir, &base)),
            )
            .await
            .map_err(|err| {
                CloudError::io("pack changes", std::io::Error::other(err.to_string()))
            })??;
        boot.fact("head", short_commit(&packed.head));
        let has_bundle = self.upload_commits(view, packed.bundle, boot).await?;
        report_uncommitted(packed.changed, boot);
        let script = workspace::start_branch(&BranchStart {
            branch,
            head: &packed.head,
            target,
            remote_url,
            bundle: has_bundle,
            // Never the hub's uncommitted work: a ship on the machine
            // commits every change it finds, and would push that too.
            delta: false,
            clean: request.clean,
        })?;
        let stdout = self
            .run_script(
                view,
                BootStage::SyncFiles,
                &format!("start the agent's branch and pull the latest {target}"),
                &script,
                boot,
            )
            .await?;
        report_pull(&stdout, boot);
        Ok(())
    }

    /// Upload the bundle of commits made since the snapshot, if any.
    /// Returns whether there was one.
    async fn upload_commits(
        &self,
        view: &MachineView,
        bundle: Option<Vec<u8>>,
        boot: &BootReporter,
    ) -> CloudResult<bool> {
        let Some(bytes) = bundle else {
            boot.fact("commits", "none since the snapshot");
            return Ok(false);
        };
        boot.fact(
            "commits",
            format!(
                "made since the snapshot, {} bundled",
                human_bytes(bytes.len() as u64)
            ),
        );
        self.upload_file(view, workspace::HEAD_BUNDLE, bytes, boot)
            .await?;
        Ok(true)
    }

    /// Upload `bytes` to `path` on the machine, showing progress.
    async fn upload_file(
        &self,
        view: &MachineView,
        path: &str,
        bytes: Vec<u8>,
        boot: &BootReporter,
    ) -> CloudResult<()> {
        let provider = self.fleet.provider();
        let machine = &view.machine.name;
        let size = human_bytes(bytes.len() as u64);
        boot.log(
            BootStage::SyncFiles,
            false,
            format!("upload {path} ({size})"),
        );
        boot.wait(
            BootStage::SyncFiles,
            format!("uploading {path} ({size}) to {machine}"),
            Some(boxd::CREATE_TIMEOUT),
            provider.upload_reported(machine, path, bytes, &mut |sent, total| {
                boot.waiting(
                    BootStage::SyncFiles,
                    snapshots::uploading(path, sent, total, machine),
                    Some(boxd::CREATE_TIMEOUT),
                );
            }),
        )
        .await?;
        Ok(())
    }

    /// Run `script` on the machine, streaming its output to `stage`.
    /// Returns its standard output.
    async fn run_script(
        &self,
        view: &MachineView,
        stage: BootStage,
        what: &str,
        script: &str,
        boot: &BootReporter,
    ) -> CloudResult<String> {
        let provider = self.fleet.provider();
        let machine = &view.machine.name;
        boot.log(stage, false, format!("$ {what}"));
        let mut lines = boot.output(stage);
        let out = boot
            .wait(
                stage,
                format!("{what} on {machine}"),
                Some(APPLY_TIMEOUT),
                provider.exec_streaming(
                    machine,
                    script,
                    None,
                    APPLY_TIMEOUT,
                    &mut |stderr, chunk| lines.chunk(stderr, chunk),
                ),
            )
            .await?;
        lines.finish();
        if out.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(CloudError::ExecFailed {
                machine: machine.clone(),
                command: what.to_string(),
                exit_code: out.exit_code,
                stderr: out.stderr_tail(400),
            })
        }
    }

    /// Give the machine's node the agent's record, page by page.
    pub(super) async fn import_record(
        &self,
        request: &PlaceRequest,
        view: &MachineView,
        boot: &BootReporter,
    ) -> CloudResult<()> {
        let node = self.node(&view.agent)?;
        let head = self.local_head(request.agent)?;
        let remote = boot
            .wait(
                BootStage::ImportRecord,
                format!("asking {} how much of the record it has", view.machine.name),
                Some(NODE_TIMEOUT),
                node.head(request.agent),
            )
            .await
            .unwrap_or(0);
        boot.fact("record", format!("{head} entries"));
        let mut from = remote + 1;
        loop {
            boot.progress(
                BootStage::ImportRecord,
                from.saturating_sub(1).min(head),
                head,
            );
            let entries = self.record_page(request.agent, from, head)?;
            let count = entries.len() as u64;
            let done = from + count > head;
            let last = (from + count).saturating_sub(1);
            boot.wait(
                BootStage::ImportRecord,
                format!(
                    "{} is importing entries {from}..={last} of {head}",
                    view.machine.name
                ),
                Some(NODE_TIMEOUT),
                node.import(&SyncImport {
                    agent_id: request.agent.to_hex(),
                    name: request.name.clone(),
                    entries,
                    done,
                }),
            )
            .await?;
            if done {
                boot.progress(BootStage::ImportRecord, head, head);
                break;
            }
            from += count;
        }
        self.upload_session(&node, request.agent, head, &view.machine.name, boot)
            .await;
        Ok(())
    }

    fn local_head(&self, agent: AgentId) -> CloudResult<u64> {
        self.store
            .get_head_seq(agent)
            .map_err(|e| CloudError::io("read record", std::io::Error::other(e.to_string())))
    }

    /// The page of `agent`'s record from `from`; empty past `head`.
    fn record_page(&self, agent: AgentId, from: u64, head: u64) -> CloudResult<Vec<RecordEntry>> {
        if from > head {
            return Ok(Vec::new());
        }
        self.store
            .scan_record(agent, from, super::node_client::PAGE)
            .map_err(|e| CloudError::io("read record", std::io::Error::other(e.to_string())))
    }
}

/// Where a new machine's workspace goes: the agent's branch, the merge
/// queue target pulled onto it, and `origin`.
#[derive(Clone, Copy)]
pub(super) struct Target<'a> {
    pub(super) branch: &'a str,
    pub(super) target: &'a str,
    pub(super) remote_url: Option<&'a str>,
}

/// Say how many uncommitted files of the hub's project stay on the hub.
/// Machines start from commits only, so no ship of theirs can push the
/// hub's unfinished work.
fn report_uncommitted(changed: usize, boot: &BootReporter) {
    let said = if changed == 0 {
        "none uncommitted".to_string()
    } else {
        format!("{changed} uncommitted file(s) stay on the hub; commit them to share them")
    };
    boot.fact("changes", said);
}

/// Show what the machine's pull from GitHub did, when it ran.
fn report_pull(stdout: &str, boot: &BootReporter) {
    if let Some(pulled) = workspace::parse_pulled(stdout) {
        boot.fact("github", pulled.to_string());
    }
}

/// The project in `dir` packed for a machine whose snapshot is at `base`.
fn pack_project(dir: &Path, base: &str) -> CloudResult<PackedProject> {
    let head = delta::head_commit(dir)?;
    let bundle = if head == base {
        None
    } else {
        delta::bundle_since(dir, base)?
    };
    let changes = delta::collect(dir, &head)?;
    let changed = changes.files.len() + changes.deleted.len();
    Ok(PackedProject {
        head,
        bundle,
        changed,
    })
}
