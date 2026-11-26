//! Pulling a cloud agent's work back: its commits as a branch here and its
//! working tree's changes as a patch (`git apply --3way`).

use std::path::Path;

use crate::delta::BranchFetch;
use crate::fleet::MachineView;
use crate::workspace::{self, Bundled};
use crate::{delta, BootReporter, CloudError, CloudResult, VmProvider};
use aura_core_types::time::now_ms;
use aura_core_types::AgentId;

use super::{
    CloudHub, CommitsOutcome, PatchOutcome, PlacedAgent, PullOutcome, GIT_SCRIPT_TIMEOUT,
    LOCAL_GIT_TIMEOUT,
};

impl CloudHub {
    /// Pull `agent`'s work into the project without moving the agent:
    /// its working tree's changes into the project folder, its commits
    /// onto the project's branch of the same name.
    ///
    /// # Errors
    ///
    /// When the node can't be reached.
    pub async fn pull(&self, agent: AgentId) -> CloudResult<PullOutcome> {
        let hex = agent.to_hex();
        let Some(placed) = self.placements.lock().agents.get(&hex).cloned() else {
            return Ok(PullOutcome::patch_only(PatchOutcome::Empty));
        };
        let node = self.awake_node(&hex, &BootReporter::none()).await?;
        let patch = node.patch().await?;
        let tree = patch.tree.clone();
        let outcome = self
            .apply_patch(&hex, &placed.project_dir, &patch.patch, patch.files)
            .await;
        if matches!(outcome, PatchOutcome::Applied { .. }) {
            // The machine keeps working; the next pull starts from what
            // this one took.
            self.mark_pulled(&hex, tree.as_deref()).await;
            self.saved_patch_pulled(agent);
        }
        let commits = self.fetch_commits(&hex, &placed).await;
        Ok(PullOutcome {
            patch: outcome,
            commits,
        })
    }

    /// Tell `hex`'s machine its patch up to `tree` is in the project.
    /// Machines from before the patch named its tree mark all they have.
    async fn mark_pulled(&self, hex: &str, tree: Option<&str>) {
        let Some(view) = self.fleet.view(hex) else {
            return;
        };
        let script = match tree {
            Some(tree) => match workspace::mark_pulled(tree) {
                Ok(script) => script,
                Err(err) => {
                    tracing::warn!(agent = %hex, error = %err, "the machine named a bad tree; the next pull repeats this one");
                    return;
                }
            },
            None => delta::MARK_PULLED_LEGACY.to_string(),
        };
        let marked = self
            .fleet
            .provider()
            .exec(&view.machine.name, &script, None, GIT_SCRIPT_TIMEOUT)
            .await;
        match marked {
            Ok(out) if out.success() => {}
            Ok(out) => {
                tracing::warn!(agent = %hex, stderr = %out.stderr_tail(300), "could not mark the pulled work on the machine; the next pull repeats it");
            }
            Err(err) => {
                tracing::warn!(agent = %hex, error = %err, "could not mark the pulled work on the machine; the next pull repeats it");
            }
        }
    }

    /// Fetch the commits on `placed`'s branch that the project here
    /// lacks into the project's branch of the same name.
    pub(super) async fn fetch_commits(&self, hex: &str, placed: &PlacedAgent) -> CommitsOutcome {
        let Some(branch) = placed.branch.clone() else {
            return CommitsOutcome::None;
        };
        let Some(view) = self.fleet.view(hex) else {
            return CommitsOutcome::None;
        };
        match self.fetch_branch(hex, placed, &view, &branch).await {
            Ok(outcome) => outcome,
            Err(err) => {
                tracing::warn!(agent = %hex, error = %err, "could not fetch the agent's commits");
                CommitsOutcome::Kept {
                    branch,
                    reason: err.to_string(),
                }
            }
        }
    }

    async fn fetch_branch(
        &self,
        hex: &str,
        placed: &PlacedAgent,
        view: &MachineView,
        branch: &str,
    ) -> CloudResult<CommitsOutcome> {
        let provider = self.fleet.provider();
        let machine = &view.machine.name;
        // The bundle leaves out what the hub last took; only while it
        // still has it.
        let fetched = still_here(placed).await;
        let Some(commits) =
            bundle_branch(provider.as_ref(), machine, branch, fetched.as_deref()).await?
        else {
            return Ok(CommitsOutcome::None);
        };
        let bytes = provider
            .download(machine, workspace::OUT_BUNDLE, delta::MAX_DELTA_BYTES)
            .await?;
        let fetched = self.fetch_bundle(hex, placed, branch, bytes).await?;
        Ok(self.branch_outcome(hex, branch.to_string(), commits, fetched))
    }

    /// Save the machine's bundle under `hex`'s folder and fetch `branch`
    /// from it into the project.
    async fn fetch_bundle(
        &self,
        hex: &str,
        placed: &PlacedAgent,
        branch: &str,
        bytes: Vec<u8>,
    ) -> CloudResult<BranchFetch> {
        let dir = self.home.join(hex);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|err| CloudError::io("save the agent's commits", err))?;
        let path = dir.join(format!("branch-{}.bundle", now_ms()));
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|err| CloudError::io("save the agent's commits", err))?;
        let project = placed.project_dir.clone();
        let bundle = path.clone();
        let name = branch.to_string();
        let fetched = tokio::time::timeout(
            LOCAL_GIT_TIMEOUT,
            tokio::task::spawn_blocking(move || delta::fetch_branch(&project, &bundle, &name)),
        )
        .await;
        let _ = tokio::fs::remove_file(&path).await;
        match fetched {
            Ok(Ok(fetched)) => fetched,
            Ok(Err(err)) => Err(CloudError::io(
                "fetch the agent's commits",
                std::io::Error::other(err.to_string()),
            )),
            Err(_) => Err(CloudError::io(
                "fetch the agent's commits",
                std::io::Error::new(std::io::ErrorKind::TimedOut, "git fetch took too long"),
            )),
        }
    }

    /// What fetching `branch` did; keeps the new tip when it moved.
    fn branch_outcome(
        &self,
        hex: &str,
        branch: String,
        commits: u64,
        fetched: BranchFetch,
    ) -> CommitsOutcome {
        match fetched {
            BranchFetch::Updated { tip } => {
                if let Some(placed) = self.placements.lock().agents.get_mut(hex) {
                    placed.fetched_tip = Some(tip);
                }
                self.save_placements();
                CommitsOutcome::Fetched { branch, commits }
            }
            BranchFetch::CheckedOut => CommitsOutcome::Kept {
                reason: format!(
                    "{branch} is checked out here; switch to another branch and pull again"
                ),
                branch,
            },
            BranchFetch::Diverged => CommitsOutcome::Kept {
                reason: format!(
                    "{branch} here has commits the agent's branch lacks; rename it and pull again"
                ),
                branch,
            },
        }
    }

    pub(super) async fn apply_patch(
        &self,
        hex: &str,
        project: &Path,
        patch: &str,
        files: Vec<String>,
    ) -> PatchOutcome {
        if patch.trim().is_empty() {
            return PatchOutcome::Empty;
        }
        let dir = self.home.join(hex);
        let path = dir.join(format!("changes-{}.patch", now_ms()));
        if let Err(err) = tokio::fs::create_dir_all(&dir).await {
            return PatchOutcome::Saved {
                path,
                reason: format!("could not save it: {err}"),
            };
        }
        if let Err(err) = tokio::fs::write(&path, patch).await {
            return PatchOutcome::Saved {
                path,
                reason: format!("could not save it: {err}"),
            };
        }
        match git_apply(project, &path).await {
            Ok(Ok(out)) if out.status.success() => {
                let _ = tokio::fs::remove_file(&path).await;
                PatchOutcome::Applied { files }
            }
            Ok(Ok(out)) => PatchOutcome::Saved {
                path,
                reason: String::from_utf8_lossy(&out.stderr)
                    .trim()
                    .chars()
                    .take(300)
                    .collect(),
            },
            Ok(Err(err)) => PatchOutcome::Saved {
                path,
                reason: format!("git apply: {err}"),
            },
            Err(_) => PatchOutcome::Saved {
                path,
                reason: "git apply took too long".into(),
            },
        }
    }
}

/// `placed`'s last fetched tip, while the project here still has it.
async fn still_here(placed: &PlacedAgent) -> Option<String> {
    let tip = placed.fetched_tip.clone()?;
    let project = placed.project_dir.clone();
    let check = tip.clone();
    tokio::task::spawn_blocking(move || delta::has_commit(&project, &check))
        .await
        .unwrap_or(false)
        .then_some(tip)
}

/// Bundle `branch` on `machine`, leaving out `fetched`. Returns how many
/// commits it bundled; `None` when there were none.
async fn bundle_branch(
    provider: &dyn VmProvider,
    machine: &str,
    branch: &str,
    fetched: Option<&str>,
) -> CloudResult<Option<u64>> {
    let script = workspace::bundle_branch(branch, fetched)?;
    let out = provider
        .exec(machine, &script, None, GIT_SCRIPT_TIMEOUT)
        .await?;
    if !out.success() {
        return Err(CloudError::ExecFailed {
            machine: machine.to_string(),
            command: "bundle the agent's branch".into(),
            exit_code: out.exit_code,
            stderr: out.stderr_tail(400),
        });
    }
    match workspace::parse_bundled(&out.stdout_text()) {
        Some(Bundled::Nothing) => Ok(None),
        Some(Bundled::Commits { commits, .. }) => Ok(Some(commits)),
        None => Err(CloudError::ExecFailed {
            machine: machine.to_string(),
            command: "bundle the agent's branch".into(),
            exit_code: out.exit_code,
            stderr: "it did not say what it bundled".into(),
        }),
    }
}

/// `git apply --3way` of the patch at `path` into `project`.
async fn git_apply(
    project: &Path,
    path: &Path,
) -> Result<std::io::Result<std::process::Output>, tokio::time::error::Elapsed> {
    tokio::time::timeout(
        LOCAL_GIT_TIMEOUT,
        tokio::process::Command::new("git")
            .arg("-C")
            .arg(project)
            .args(["apply", "--3way", "--whitespace=nowarn"])
            .arg(path)
            .kill_on_drop(true)
            .output(),
    )
    .await
}
