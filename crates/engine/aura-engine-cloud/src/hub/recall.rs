//! Recalling a placed agent from its machine.

use std::sync::Arc;
use std::time::Duration;

use crate::{names, BootReporter, BootStage, CloudError, CloudResult};
use aura_core_types::time::{format_duration, now_ms};
use aura_core_types::{AgentId, Placement, PlacementPayload};

use super::node_client::NodeClient;
use super::store::record_placement;

use super::{CloudHub, CommitsOutcome, PatchOutcome, PlacedAgent, PullOutcome};

impl CloudHub {
    /// Bring `agent`'s single writer back here: seal it on its node,
    /// copy the rest of its record, pull its work into the project and
    /// destroy the machine, showing each step on `boot`.
    ///
    /// # Errors
    ///
    /// When the node can't be sealed or its record can't be copied; the
    /// agent then stays placed.
    pub async fn recall(
        &self,
        agent: AgentId,
        reason: &str,
        boot: &BootReporter,
    ) -> CloudResult<PullOutcome> {
        let hex = agent.to_hex();
        let Some(placed) = self.placements.lock().agents.get(&hex).cloned() else {
            return Ok(PullOutcome::patch_only(PatchOutcome::Empty));
        };
        boot.fact("machine", placed.machine.clone());
        boot.fact("project folder", placed.project_dir.display().to_string());
        if self.fleet.view(&hex).is_none() {
            return Ok(self.recall_without_machine(&hex, &placed, boot).await);
        }
        boot.plan(&[
            BootStage::Seal,
            BootStage::CopyRecord,
            BootStage::PullChanges,
            BootStage::Destroy,
        ]);
        let node = self.awake_node(&hex, boot).await?;
        let sealed = boot.run(BootStage::Seal, node.seal(agent)).await?;
        boot.fact("sealed at", format!("entry {}", sealed.head_seq));
        boot.run(
            BootStage::CopyRecord,
            self.copy_rest(&node, agent, sealed.head_seq, boot),
        )
        .await?;
        let outcome = boot
            .run(BootStage::PullChanges, async {
                Ok::<_, CloudError>(self.pull_back(&node, &hex, &placed, boot).await)
            })
            .await?;
        record_placement(
            &self.ledger,
            &names::machine_ledger(&placed.zns),
            &PlacementPayload::new(
                Placement::Cloud,
                Placement::Local,
                Some(placed.machine.clone()),
            ),
        )?;
        self.forget_placement(&hex, &placed.project);
        self.destroy_recalled(&hex, &placed, reason, boot).await?;
        Ok(outcome)
    }

    /// Recall `hex` when its machine is gone (deleted outside aura): what
    /// was copied already is all there is, with the changes saved at its
    /// last sleep.
    async fn recall_without_machine(
        &self,
        hex: &str,
        placed: &PlacedAgent,
        boot: &BootReporter,
    ) -> PullOutcome {
        let outcome = match self.saved_patch(hex) {
            Some(saved) => {
                boot.fact(
                    "changes from",
                    format!(
                        "its last sleep, {} ago",
                        format_duration(now_ms().saturating_sub(saved.saved_ms) / 1000)
                    ),
                );
                self.apply_patch(
                    hex,
                    &placed.project_dir,
                    &saved.patch.patch,
                    saved.patch.files,
                )
                .await
            }
            None => PatchOutcome::Saved {
                path: self.home.join(hex),
                reason: format!(
                    "machine {} no longer exists; its unsynced work is lost",
                    placed.machine
                ),
            },
        };
        self.forget_placement(hex, &placed.project);
        PullOutcome::patch_only(outcome)
    }

    /// Copy the rest of `agent`'s record from its sealed node, up to
    /// `sealed_head`.
    async fn copy_rest(
        &self,
        node: &NodeClient,
        agent: AgentId,
        sealed_head: u64,
        boot: &BootReporter,
    ) -> CloudResult<()> {
        let copied = self.mirror_from(node, agent).await?;
        boot.fact("copied", format!("{copied} new entries"));
        let local = self
            .store
            .get_head_seq(agent)
            .map_err(|e| CloudError::io("read record", std::io::Error::other(e.to_string())))?;
        boot.progress(BootStage::CopyRecord, local.min(sealed_head), sealed_head);
        if local < sealed_head {
            return Err(CloudError::Node {
                node: node.node().to_string(),
                op: "recall",
                message: format!("copied up to {local} of {sealed_head} entries"),
            });
        }
        // The whole session and its runs, so the agent resumes here
        // with its tool calls; without them it resumes from the record.
        match self.copy_session(node, agent).await {
            Ok(true) => boot.fact("session", "copied"),
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(agent = %agent.to_hex(), error = %err, "could not copy the session checkpoint");
            }
        }
        if let Err(err) = self.copy_traces(node, agent).await {
            tracing::warn!(agent = %agent.to_hex(), error = %err, "could not copy the trace runs");
        }
        Ok(())
    }

    /// Pull the recalled agent's changes and commits into the project.
    async fn pull_back(
        &self,
        node: &NodeClient,
        hex: &str,
        placed: &PlacedAgent,
        boot: &BootReporter,
    ) -> PullOutcome {
        let patch = match node.patch().await {
            Ok(patch) => {
                self.apply_patch(hex, &placed.project_dir, &patch.patch, patch.files)
                    .await
            }
            Err(err) => PatchOutcome::Saved {
                path: self.home.join(hex),
                reason: format!("could not read the machine's changes: {err}"),
            },
        };
        if let PatchOutcome::Applied { files } = &patch {
            for file in files {
                boot.log(BootStage::PullChanges, false, format!("M {file}"));
            }
        }
        boot.fact("changes", patch.describe());
        let commits = self.fetch_commits(hex, placed).await;
        if let Some(report) = commits.describe() {
            let kept = matches!(commits, CommitsOutcome::Kept { .. });
            boot.log(BootStage::PullChanges, kept, report.clone());
            boot.fact("commits", report);
        }
        PullOutcome { patch, commits }
    }

    /// Destroy the recalled agent's machine and settle its usage.
    async fn destroy_recalled(
        &self,
        hex: &str,
        placed: &PlacedAgent,
        reason: &str,
        boot: &BootReporter,
    ) -> CloudResult<()> {
        let destroyed = boot
            .run(BootStage::Destroy, async {
                Ok::<_, CloudError>(self.fleet.destroy(hex, reason).await)
            })
            .await?;
        match destroyed {
            Ok(Some(id)) => {
                let fleet = Arc::clone(&self.fleet);
                let ledger = names::machine_ledger(&placed.zns);
                let machine = placed.machine.clone();
                tokio::spawn(async move {
                    fleet
                        .settle_usage(&id, &machine, &ledger, 6, Duration::from_secs(300))
                        .await;
                });
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(agent = %hex, error = %err, "could not destroy the machine; the orphan sweep will");
                boot.log(
                    BootStage::Destroy,
                    true,
                    format!("could not destroy it ({err}); the orphan sweep will"),
                );
            }
        }
        Ok(())
    }

    fn forget_placement(&self, hex: &str, project: &str) {
        self.placements.lock().agents.remove(hex);
        self.save_placements();
        self.forget_saved_patch(hex);
        if let Ok(agent) = AgentId::from_hex(hex) {
            self.sync.lock().remove(&agent);
        }
        self.directory.remove(hex);
        self.send_directories(project);
        if let Some(link) = self.links.lock().remove(hex) {
            link.stop.cancel();
        }
        self.tracks.lock().remove(hex);
    }
}
