//! Placing an agent on a cloud machine: its project snapshot and GitHub
//! repository share.

use std::sync::Arc;

use crate::fleet::{MachineView, ProvisionRequest};
use crate::github::{BotIdentity, RepoRef};
use crate::machine_env::{MachineEnv, WORKSPACE_DIR};
use crate::snapshots::SnapshotRecord;
use crate::workspace::{self};
use crate::{delta, names, BootReporter, BootStage, CloudError, CloudResult, MachineOrigin};
use aura_core_protocol::cloud::GitClaim;
use aura_core_types::{Placement, PlacementPayload};

use super::store::record_placement;

use super::{CloudHub, PlaceRequest, PlacedAgent, RepoShare};

impl CloudHub {
    /// Move `request.agent`'s single writer to a cloud machine, showing
    /// each step on `boot`. The caller must hold the agent idle until
    /// this returns.
    ///
    /// # Errors
    ///
    /// When the snapshot, the machine or the record import fails; the
    /// agent then stays here and any machine made for it is destroyed.
    pub async fn place(
        self: &Arc<Self>,
        request: PlaceRequest,
        boot: &BootReporter,
    ) -> CloudResult<MachineView> {
        let hex = request.agent.to_hex();
        if self.is_placed(request.agent) {
            return self.fleet.view(&hex).ok_or_else(|| CloudError::NotFound {
                what: "machine",
                name: hex.clone(),
            });
        }
        // A fork inherits its parent's plan secrets: only within one user.
        let parent = request
            .fork_from
            .filter(|parent| self.owner_of(*parent).as_ref() == Some(&request.owner))
            .map(|parent| parent.to_hex())
            .filter(|parent| self.config.fork_on_spawn && self.fleet.has(parent));
        let owner_tag = names::owner_tag(&request.owner);
        boot.fact("agent", request.name.clone());
        boot.plan(&[
            BootStage::Plans,
            BootStage::Repo,
            BootStage::Snapshot,
            BootStage::Quota,
            BootStage::Create,
            BootStage::Configure,
            BootStage::Claim,
            BootStage::SyncFiles,
            BootStage::ImportRecord,
        ]);
        let (secrets, plans) = match self.plan_sync() {
            Some(sync) => {
                boot.run(BootStage::Plans, async {
                    let ensured = boot
                        .wait(
                            BootStage::Plans,
                            "boxd is storing the plan sign-ins as machine secrets",
                            None,
                            sync.ensure_owner(&request.owner),
                        )
                        .await;
                    Ok::<_, CloudError>(match ensured {
                        Ok(secrets) => {
                            let plans = sync.templates(&request.owner);
                            boot.fact("plans", format!("{} plan sign-in(s) shared", plans.len()));
                            (secrets, plans)
                        }
                        Err(err) => {
                            tracing::warn!(owner = %owner_tag, error = %err, "could not set the plan secrets; the machine gets no plans");
                            boot.log(BootStage::Plans, true, format!("no plans for this machine: {err}"));
                            (Vec::new(), Vec::new())
                        }
                    })
                })
                .await?
            }
            None => {
                boot.skip(BootStage::Plans, "no plan sign-ins here");
                (Vec::new(), Vec::new())
            }
        };
        let branch = workspace::agent_branch(&request.name, request.code.as_deref(), &hex);
        boot.fact("branch", branch.clone());
        let github = self.share_repo(&request, boot).await?;
        let git = Self::git_claim(
            Some(&branch),
            github.repo.as_ref(),
            github.secret.clone(),
            github.identity.as_ref(),
        );
        let (origin, base_commit, seal) = match parent.as_deref().and_then(|p| self.fleet.view(p)) {
            Some(parent_view) => {
                boot.skip(
                    BootStage::Snapshot,
                    format!("forking the parent's machine {}", parent_view.machine.name),
                );
                (
                    MachineOrigin::Fork(parent_view.machine.name.clone()),
                    None,
                    vec![parent_view.agent.clone()],
                )
            }
            None => {
                let record = boot
                    .run(BootStage::Snapshot, self.project_snapshot(&request, boot))
                    .await?;
                (
                    MachineOrigin::Snapshot(record.name),
                    record.commit,
                    Vec::new(),
                )
            }
        };
        let view = self
            .fleet
            .provision_reported(
                ProvisionRequest {
                    agent: hex.clone(),
                    agent_zns: request.agent_zns.clone(),
                    code: request.code.clone(),
                    project: request.project_id.clone(),
                    name: names::machine_name(request.code.as_deref(), &request.agent),
                    origin,
                    seal,
                    owner: owner_tag.clone(),
                    secrets,
                    plans,
                    repo_secret: github.secret.clone(),
                    git,
                },
                boot,
            )
            .await?;
        let env = MachineEnv::for_agent(view.machine.vcpu).into_map();
        boot.fact("workspace", WORKSPACE_DIR);
        boot.fact(
            "env",
            format!(
                "{} (allowlist)",
                env.keys().map(String::as_str).collect::<Vec<_>>().join(" ")
            ),
        );
        let remote_url = github.repo.as_ref().map(RepoRef::https_url);
        if let Err(err) = self
            .seed(
                &request,
                &view,
                base_commit.as_deref(),
                &branch,
                remote_url.as_deref(),
                boot,
            )
            .await
        {
            tracing::warn!(agent = %hex, error = %err, "could not move the agent to its machine; destroying it");
            let _ = self.fleet.destroy(&hex, "placement failed").await;
            return Err(err);
        }
        let placed = PlacedAgent {
            zns: request.agent_zns.clone(),
            name: request.name.clone(),
            project: request.project_id.clone(),
            project_dir: request.project_dir.clone(),
            machine: view.machine.name.clone(),
            owner: Some(request.owner.clone()),
            owner_name: request.owner_name.clone(),
            branch: Some(branch),
            repo: github.repo,
            fetched_tip: None,
        };
        record_placement(
            &self.ledger,
            &names::machine_ledger(&request.agent_zns),
            &PlacementPayload::new(
                Placement::Local,
                Placement::Cloud,
                Some(view.machine.name.clone()),
            )
            .with_owner(Some(owner_tag)),
        )?;
        self.placements
            .lock()
            .agents
            .insert(hex.clone(), placed.clone());
        self.save_placements();
        self.place_in_directory(&hex, &placed, &view);
        self.open_link(&hex);
        Ok(self.fleet.view(&hex).unwrap_or(view))
    }

    /// The project snapshot a new machine starts from, rebuilt from HEAD
    /// when this repository no longer has the snapshot's commit (the
    /// delta is taken against it).
    async fn project_snapshot(
        &self,
        request: &PlaceRequest,
        boot: &BootReporter,
    ) -> CloudResult<SnapshotRecord> {
        let ensure = |force| {
            self.snapshots.ensure_project_reported(
                &request.project_id,
                &request.project_dir,
                request.build_command.as_deref(),
                force,
                boot,
            )
        };
        let record = ensure(false).await?;
        let missing = match record.commit.clone() {
            Some(commit) => {
                let dir = request.project_dir.clone();
                !tokio::task::spawn_blocking(move || delta::has_commit(&dir, &commit))
                    .await
                    .unwrap_or(false)
            }
            None => false,
        };
        if !missing {
            return Ok(record);
        }
        tracing::info!(project = %request.project_id, "the project snapshot's commit is gone here; rebuilding it");
        boot.log(
            BootStage::Snapshot,
            false,
            "the snapshot's commit is gone here; rebuilding it from HEAD",
        );
        ensure(true).await
    }

    /// Hand the project's GitHub token to boxd as the repository's
    /// secret. Never fails placement: without a token the agent still
    /// commits on its branch, and pulls bring the commits back.
    pub(super) async fn share_repo(
        &self,
        request: &PlaceRequest,
        boot: &BootReporter,
    ) -> CloudResult<RepoShare> {
        let dir = request.project_dir.clone();
        let url = tokio::task::spawn_blocking(move || delta::origin_url(&dir))
            .await
            .ok()
            .flatten();
        let Some(repo) = url.as_deref().and_then(RepoRef::from_remote) else {
            let why = if url.is_some() {
                "origin is not on GitHub"
            } else {
                "the project has no origin"
            };
            boot.skip(
                BootStage::Repo,
                format!("{why}: the agent's commits come back with pulls"),
            );
            return Ok(RepoShare::default());
        };
        boot.run(BootStage::Repo, async {
            boot.fact("repository", repo.full_name());
            let sync = self.repo_sync();
            let ensured = boot
                .wait(
                    BootStage::Repo,
                    format!("boxd is storing the GitHub token for {repo} as a machine secret"),
                    None,
                    sync.ensure(&repo),
                )
                .await;
            let status = match ensured {
                Ok(status) => status,
                Err(err) => {
                    tracing::warn!(repo = %repo, error = %err, "could not set the repository secret; the machine gets no GitHub access");
                    boot.log(
                        BootStage::Repo,
                        true,
                        format!("no GitHub access for this machine: {err}"),
                    );
                    return Ok(RepoShare {
                        repo: Some(repo.clone()),
                        ..RepoShare::default()
                    });
                }
            };
            if status.linked {
                boot.fact("github", "token shared; the App's bot pushes and opens pull requests");
            } else {
                let notice = match (&status.install_url, &status.error) {
                    (Some(url), _) => format!("GitHub App not installed on {repo}: {url}"),
                    (None, Some(error)) => format!("no GitHub token for {repo}: {error}"),
                    (None, None) => format!("no GitHub token for {repo}"),
                };
                boot.log(BootStage::Repo, true, notice);
                boot.fact("github", "no token; the agent's commits come back with pulls");
            }
            // Only ask GitHub who the bot is when it answered just now.
            let identity = if status.linked || status.install_url.is_some() {
                sync.identity().await
            } else {
                None
            };
            Ok::<_, CloudError>(RepoShare {
                repo: Some(repo.clone()),
                secret: Some(status.secret),
                identity,
            })
        })
        .await
    }

    /// What a node is told about git: the agent's branch, and the
    /// repository it pushes to as the App's bot.
    pub(super) fn git_claim(
        branch: Option<&str>,
        repo: Option<&RepoRef>,
        secret: Option<String>,
        identity: Option<&BotIdentity>,
    ) -> Option<GitClaim> {
        Some(GitClaim {
            branch: branch?.to_string(),
            remote_url: repo.map(RepoRef::https_url),
            env: secret,
            placeholder: None,
            author_name: identity.map(BotIdentity::login),
            author_email: identity.map(BotIdentity::email),
        })
    }
}
