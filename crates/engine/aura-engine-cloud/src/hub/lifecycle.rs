//! Building the hub, starting it on the machines it had and stopping it,
//! and the fleet's timers.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Weak};

use crate::cost::CostModel;
use crate::fleet::VmFleet;
use crate::github::{BotIdentity, GithubApp, RepoTokens};
use crate::mesh::{Directory, MailBook};
use crate::plans::SYNC_EVERY;
use crate::repos::RepoSync;
use crate::snapshots::{AuraBinary, SnapshotBuilder, SnapshotSettings};
use crate::{boxd, BootReporter, BootStage, CloudResult, VmProvider};
use aura_config::CloudConfig;
use aura_core_auth::Owner;
use aura_store_db::Store;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use super::store::StoreLedger;

use super::{
    choose_binary, io, load_placements, CloudHub, PlacedAgent, PlacementFile, POLL_EVERY,
    SWEEP_EVERY, TICK_EVERY, USAGE_EVERY,
};

impl CloudHub {
    /// A hub over `provider`, keeping its files in `home`
    /// (`AURA_HOME/cloud`).
    ///
    /// # Errors
    ///
    /// When `home` can't be created.
    pub fn new(
        config: CloudConfig,
        store: Arc<dyn Store>,
        home: PathBuf,
        provider: Arc<dyn VmProvider>,
        binary: AuraBinary,
    ) -> CloudResult<Arc<Self>> {
        let placements = open_placements(&home)?;
        let ledger = Arc::new(StoreLedger::new(Arc::clone(&store)));
        let parts = MachineParts::new(&config, &ledger, provider, binary, &home, &placements);
        let hub = Arc::new(Self {
            costs: CostModel::from_config(&config),
            config,
            store,
            ledger,
            fleet: parts.fleet,
            snapshots: parts.snapshots,
            directory: Directory::new(),
            mail: MailBook::new(),
            home,
            placements: Mutex::new(placements),
            host: Mutex::new(None),
            desk: Mutex::new(None),
            reach: Mutex::new(HashMap::new()),
            links: Mutex::new(HashMap::new()),
            tracks: Mutex::new(HashMap::new()),
            mirrors: Mutex::new(HashMap::new()),
            copies: Mutex::new(HashMap::new()),
            observer: Mutex::new(None),
            copied: AtomicBool::new(false),
            trace_store: Mutex::new(None),
            run_marks: Mutex::new(HashMap::new()),
            traces_copied: Mutex::new(HashSet::new()),
            sync: Mutex::new(HashMap::new()),
            plans: Mutex::new(None),
            repos: Mutex::new(parts.repos),
            sessions: Mutex::new(None),
            current_owner: Mutex::new(Owner::Local),
            work: Mutex::new(HashMap::new()),
            streaming: Mutex::new(HashMap::new()),
            stop: CancellationToken::new(),
        });
        hub.save_placements();
        Ok(hub)
    }

    /// A hub over boxd itself, from the cloud settings.
    ///
    /// # Errors
    ///
    /// [`CloudError::NotConfigured`] without a key; otherwise as
    /// [`Self::new`].
    pub fn from_config(
        config: CloudConfig,
        store: Arc<dyn Store>,
        home: PathBuf,
    ) -> CloudResult<Arc<Self>> {
        let provider = Arc::new(crate::BoxdProvider::from_config(&config)?);
        let binary = if cfg!(target_os = "linux") {
            choose_binary(
                config.binary.as_deref(),
                &home,
                std::env::current_exe().ok(),
            )
        } else {
            AuraBinary::Installed
        };
        Self::new(config, store, home, provider, binary)
    }

    /// Take back the machines placed agents had before this hub started,
    /// destroy machines no kept agent owns, and start the fleet's
    /// timers. `keep` are the agents that still exist (hex). What the
    /// machines made while no hub watched is copied in the background;
    /// the turns they were asked for are followed with
    /// [`Self::follow_turn`] ([`Self::unfinished_turns`]).
    pub async fn start(self: &Arc<Self>, keep: &HashSet<String>) {
        self.start_reported(keep, &BootReporter::none()).await;
    }

    /// [`Self::start`], showing on `boot` each machine it takes back and
    /// the sweep, as the [`BootStage::Connect`] stage's waits.
    pub async fn start_reported(self: &Arc<Self>, keep: &HashSet<String>, boot: &BootReporter) {
        let placed = self.placed();
        let kept = placed.iter().filter(|(hex, _)| keep.contains(hex)).count();
        if kept > 0 {
            boot.log(
                BootStage::Connect,
                false,
                format!("taking back {kept} cloud machine(s)"),
            );
        }
        let repos = self.repo_sync();
        let identity = if placed.iter().any(|(_, agent)| agent.repo.is_some()) {
            repos.identity().await
        } else {
            None
        };
        for (hex, agent) in &placed {
            if !keep.contains(hex) {
                continue;
            }
            self.adopt_placed(hex, agent, &repos, identity.as_ref(), boot)
                .await;
        }
        self.save_placements();
        let owned: HashSet<String> = placed
            .iter()
            .filter(|(hex, _)| keep.contains(hex))
            .map(|(hex, _)| hex.clone())
            .collect();
        self.sweep_unowned(&owned, boot).await;
        for (hex, _) in self.placed() {
            self.open_link(&hex);
        }
        let hub = Arc::clone(self);
        tokio::spawn(async move { hub.catch_up().await });
        let hub = Arc::downgrade(self);
        let stop = self.stop.clone();
        tokio::spawn(async move { maintain(hub, stop).await });
    }

    /// Take back `hex`'s machine, or run it here again when the machine
    /// is gone.
    async fn adopt_placed(
        &self,
        hex: &str,
        agent: &PlacedAgent,
        repos: &RepoSync,
        identity: Option<&BotIdentity>,
        boot: &BootReporter,
    ) {
        let templates = self
            .plan_sync()
            .map(|plans| plans.templates(&agent.owner()))
            .unwrap_or_default();
        let git = Self::git_claim(
            agent.branch.as_deref(),
            agent.repo.as_ref(),
            agent.repo.as_ref().map(|repo| repos.secret_name(repo)),
            identity,
        );
        let adopted = boot
            .wait(
                BootStage::Connect,
                format!("boxd is looking up {} ({})", agent.machine, agent.name),
                Some(boxd::CALL_TIMEOUT),
                self.fleet.adopt(
                    hex,
                    &agent.zns,
                    &agent.project,
                    &agent.machine,
                    templates,
                    git,
                ),
            )
            .await;
        match adopted {
            Ok(view) => {
                boot.log(
                    BootStage::Connect,
                    false,
                    format!("{} is {}", agent.machine, view.state.label()),
                );
                self.place_in_directory(hex, agent, &view);
            }
            Err(err) => {
                tracing::warn!(agent = %hex, machine = %agent.machine, error = %err, "cloud machine is gone; the agent runs here again");
                boot.log(
                    BootStage::Connect,
                    true,
                    format!("{} is gone; {} runs here again", agent.machine, agent.name),
                );
                self.placements.lock().agents.remove(hex);
            }
        }
    }

    /// Destroy this install's machines that no agent in `owned` owns.
    async fn sweep_unowned(&self, owned: &HashSet<String>, boot: &BootReporter) {
        let swept = boot
            .wait(
                BootStage::Connect,
                "boxd is listing this install's machines, to destroy those no agent owns",
                Some(boxd::CALL_TIMEOUT),
                self.fleet.sweep_orphans(owned),
            )
            .await;
        match swept {
            Ok(swept) if !swept.is_empty() => {
                tracing::info!(machines = ?swept, "destroyed cloud machines no agent owns");
                boot.log(
                    BootStage::Connect,
                    false,
                    format!("destroyed {} machine(s) no agent owns", swept.len()),
                );
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(error = %err, "could not list cloud machines");
                boot.log(
                    BootStage::Connect,
                    true,
                    format!("could not list cloud machines: {err}"),
                );
            }
        }
    }

    /// Stop the timers and close every socket. Machines keep running, and
    /// the turns on them: the next hub follows those to their end
    /// ([`Self::unfinished_turns`]). An idle machine hibernates on boxd's
    /// backstop ([`crate::fleet::AUTO_HIBERNATE_BACKSTOP_SECS`]).
    pub fn shutdown(&self) {
        self.stop.cancel();
        for (_, link) in self.links.lock().drain() {
            link.stop.cancel();
        }
    }
}

/// The parts of a hub that make and keep this install's machines, all
/// writing their rows through one ledger.
struct MachineParts {
    fleet: Arc<VmFleet>,
    repos: Arc<RepoSync>,
    snapshots: SnapshotBuilder,
}

impl MachineParts {
    fn new(
        config: &CloudConfig,
        ledger: &Arc<StoreLedger>,
        provider: Arc<dyn VmProvider>,
        binary: AuraBinary,
        home: &Path,
        placements: &PlacementFile,
    ) -> Self {
        let settings = crate::fleet::FleetSettings::from_config(config, placements.install.clone());
        let fleet = Arc::new(VmFleet::new(
            Arc::clone(&provider),
            Arc::clone(ledger) as Arc<dyn crate::fleet::VmLedger>,
            settings,
        ));
        let repos = Arc::new(RepoSync::new(
            Arc::clone(&provider),
            github_tokens(config),
            placements.install.clone(),
        ));
        let snapshots = SnapshotBuilder::new(
            provider,
            Arc::clone(ledger) as Arc<dyn crate::fleet::VmLedger>,
            SnapshotSettings {
                install: placements.install.clone(),
                size: config.size,
                isolated: config.isolated,
                aura_version: env!("CARGO_PKG_VERSION").to_string(),
                binary,
            },
            home.to_path_buf(),
        );
        Self {
            fleet,
            repos,
            snapshots,
        }
    }
}

/// The placements file in `home`, created with this install's id when
/// there is none.
fn open_placements(home: &Path) -> CloudResult<PlacementFile> {
    std::fs::create_dir_all(home).map_err(io(format!("create {}", home.display())))?;
    let placements_path = home.join("placements.json");
    let mut placements = load_placements(&placements_path);
    if placements.install.is_empty() {
        placements.install = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    }
    Ok(placements)
}

/// The GitHub App's tokens, when one is configured and usable.
fn github_tokens(config: &CloudConfig) -> Option<Arc<dyn RepoTokens>> {
    config.github_app.clone().and_then(|app| match GithubApp::new(app) {
        Ok(app) => Some(Arc::new(app) as Arc<dyn RepoTokens>),
        Err(err) => {
            tracing::warn!(error = %err, "the GitHub App can't be used; cloud agents get no GitHub access");
            None
        }
    })
}

/// The fleet's timers.
async fn maintain(hub: Weak<CloudHub>, stop: CancellationToken) {
    let mut tick = tokio::time::interval(TICK_EVERY);
    let mut poll = tokio::time::interval(POLL_EVERY);
    let mut usage = tokio::time::interval(USAGE_EVERY);
    let mut plans = tokio::time::interval(SYNC_EVERY);
    let mut sweep = tokio::time::interval(SWEEP_EVERY);
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = tick.tick() => {
                let Some(hub) = hub.upgrade() else { return };
                hub.sleep_idle_machines().await;
            }
            _ = sweep.tick() => {
                let Some(hub) = hub.upgrade() else { return };
                hub.sweep();
            }
            _ = poll.tick() => {
                let Some(hub) = hub.upgrade() else { return };
                for agent in hub.fleet.poll().await {
                    hub.open_link(&agent);
                }
            }
            _ = usage.tick() => {
                let Some(hub) = hub.upgrade() else { return };
                hub.fleet.record_usage().await;
            }
            _ = plans.tick() => {
                let Some(hub) = hub.upgrade() else { return };
                if let Some(sync) = hub.plan_sync() {
                    sync.tick(&hub.owners()).await;
                }
                let repos = hub.placed_repos();
                if !repos.is_empty() {
                    hub.repo_sync().tick(&repos).await;
                }
            }
        }
    }
}
