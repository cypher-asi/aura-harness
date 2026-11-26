//! Mirroring a placed agent's record and recalling it from its machine.

use std::sync::Arc;
use std::time::Duration;

use crate::{names, replica, BootReporter, BootStage, CloudError, CloudResult};
use aura_core_types::{AgentId, Placement, PlacementPayload};

use super::node_client::NodeClient;
use super::store::{record_placement, StoreSink};

use super::{CloudHub, CommitsOutcome, PatchOutcome, PullOutcome};

impl CloudHub {
    /// Copy what `agent`'s node committed since the local head. Returns
    /// the entries copied.
    ///
    /// # Errors
    ///
    /// When the node can't be read or a page fails its checks.
    pub async fn mirror(&self, agent: AgentId) -> CloudResult<usize> {
        let node = self.node(&agent.to_hex())?;
        self.mirror_from(&node, agent).await
    }

    pub(super) async fn mirror_from(
        &self,
        node: &NodeClient,
        agent: AgentId,
    ) -> CloudResult<usize> {
        // One mirror per agent at a time: two racing copies of the same
        // page would have the loser refused by the store.
        let gate = Arc::clone(self.mirrors.lock().entry(agent).or_default());
        let _one = gate.lock().await;
        let sink = StoreSink(Arc::clone(&self.store));
        let mut copied = 0;
        loop {
            let head = replica::ReplicaSink::head(&sink, agent)?;
            let page = node.record(agent, head + 1).await?;
            if page.is_empty() {
                return Ok(copied);
            }
            let applied = replica::apply_page(&sink, agent, &page)?;
            copied += applied;
            if applied == 0 {
                return Ok(copied);
            }
        }
    }

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
            // The machine is gone (deleted outside aura): the record the
            // mirror already holds is all there is.
            self.forget_placement(&hex, &placed.project);
            return Ok(PullOutcome::patch_only(PatchOutcome::Saved {
                path: self.home.join(&hex),
                reason: format!(
                    "machine {} no longer exists; its unsynced work is lost",
                    placed.machine
                ),
            }));
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
        boot.run(BootStage::CopyRecord, async {
            let copied = self.mirror_from(&node, agent).await?;
            boot.fact("copied", format!("{copied} new entries"));
            let local = self
                .store
                .get_head_seq(agent)
                .map_err(|e| CloudError::io("read record", std::io::Error::other(e.to_string())))?;
            boot.progress(
                BootStage::CopyRecord,
                local.min(sealed.head_seq),
                sealed.head_seq,
            );
            if local < sealed.head_seq {
                return Err(CloudError::Node {
                    node: node.node().to_string(),
                    op: "recall",
                    message: format!("copied up to {local} of {} entries", sealed.head_seq),
                });
            }
            Ok(())
        })
        .await?;
        let outcome = boot
            .run(BootStage::PullChanges, async {
                let patch = match node.patch().await {
                    Ok(patch) => {
                        self.apply_patch(&hex, &placed.project_dir, &patch.patch, patch.files)
                            .await
                    }
                    Err(err) => PatchOutcome::Saved {
                        path: self.home.join(&hex),
                        reason: format!("could not read the machine's changes: {err}"),
                    },
                };
                if let PatchOutcome::Applied { files } = &patch {
                    for file in files {
                        boot.log(BootStage::PullChanges, false, format!("M {file}"));
                    }
                }
                boot.fact("changes", patch.describe());
                let commits = self.fetch_commits(&hex, &placed).await;
                if let Some(report) = commits.describe() {
                    let kept = matches!(commits, CommitsOutcome::Kept { .. });
                    boot.log(BootStage::PullChanges, kept, report.clone());
                    boot.fact("commits", report);
                }
                Ok::<_, CloudError>(PullOutcome { patch, commits })
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
        let destroyed = boot
            .run(BootStage::Destroy, async {
                Ok::<_, CloudError>(self.fleet.destroy(&hex, reason).await)
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
        Ok(outcome)
    }

    fn forget_placement(&self, hex: &str, project: &str) {
        self.placements.lock().agents.remove(hex);
        self.save_placements();
        self.directory.remove(hex);
        self.send_directories(project);
        if let Some(link) = self.links.lock().remove(hex) {
            link.stop.cancel();
        }
        self.tracks.lock().remove(hex);
    }
}
