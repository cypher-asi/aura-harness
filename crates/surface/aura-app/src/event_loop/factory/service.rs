//! The hub's merge queues: one per repo, each with its processor, fed by
//! every push an agent makes ([`super::observe`]).
//!
//! ## Invariants
//!
//! - A repo has one queue and one processor in this process, keyed by
//!   its common git dir, so every worktree and clone path of a repo
//!   shares them. The queue's file lock keeps other processes out.
//! - Queues of the channels' projects that have a queue file are opened
//!   at start, which recovers what a crash left and resumes processing.
//! - A failed merge goes back to the agent that pushed it as a notice,
//!   which starts a turn for it to fix and ship again; every change goes
//!   to the QUEUE tab of each channel on the repo.

use super::notes::{
    broken_target_prompt, failure_prompt, held_line, outcome_line, queued_note, Named,
};
use super::observe::{self, ObservedPush};
use super::pages::{self, Windows};
use super::repos::{
    channel_folders, channels_on, coordinators_on, forge_of, has_queue_file, project_folders,
};
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::handlers::system_message;
use crate::event_loop::notices::Notice;
use crate::event_loop::subagents::{lock, AgentHub};
use aura_core_types::AgentId;
use aura_exec_merge_queue::{
    Enqueued, GitIntegrator, Hold, HoldKind, Limits, MergeEntry, MergeQueue, MergeRequest,
    MergeStatus, Origin, QueueConfig, QueueEvent, RepoPaths,
};
use aura_ui::UiCommand;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// One repo's queue and what it merges into.
struct RepoQueue {
    queue: MergeQueue,
    target: String,
}

/// Every repo's queue.
pub(crate) struct QueueService {
    hub: Arc<AgentHub>,
    commands: mpsc::Sender<UiCommand>,
    quit: CancellationToken,
    queues: Mutex<HashMap<PathBuf, Arc<RepoQueue>>>,
    /// How much history each channel's QUEUE tab shows
    windows: Windows,
    /// The broken target tips reported already, once each
    reported: std::sync::Mutex<std::collections::HashSet<String>>,
}

/// Start the merge queues: hear every push from now on, and resume the
/// queues the channels' projects have. Does nothing when they were
/// started already in this process.
pub(crate) fn start(
    hub: Arc<AgentHub>,
    commands: mpsc::Sender<UiCommand>,
    quit: CancellationToken,
) {
    let (sink, pushes) = mpsc::unbounded_channel();
    if !observe::install(sink) {
        return;
    }
    let service = QueueService::new(hub, commands, quit);
    // `observe::install` succeeded above, which happens once per process,
    // so this is the first and only set; a second would be a bug.
    if SERVICE.set(Arc::clone(&service)).is_err() {
        warn!("The merge queue service was started twice; the first one stays");
        return;
    }
    tokio::spawn(service.run(pushes));
}

/// The running service, for the channels opened from now on.
static SERVICE: std::sync::OnceLock<Arc<QueueService>> = std::sync::OnceLock::new();

/// Channel `channel_id`'s QUEUE tab reached its end with older merges not
/// shown: send it another page of them.
pub(crate) fn load_older(channel_id: &str) {
    let Some(service) = SERVICE.get() else {
        return;
    };
    service.windows.widen(channel_id);
    show_channel(channel_id);
}

/// Channel `channel_id` opened: send its QUEUE tab its repo's queue, when
/// the repo has one.
pub(crate) fn show_channel(channel_id: &str) {
    let Some(service) = SERVICE.get() else {
        return;
    };
    let service = Arc::clone(service);
    let channel_id = channel_id.to_string();
    tokio::spawn(async move { service.show(&channel_id).await });
}

impl QueueService {
    /// A service with no queue open yet.
    pub(crate) fn new(
        hub: Arc<AgentHub>,
        commands: mpsc::Sender<UiCommand>,
        quit: CancellationToken,
    ) -> Arc<Self> {
        Arc::new(Self {
            hub,
            commands,
            quit,
            queues: Mutex::default(),
            windows: Windows::default(),
            reported: std::sync::Mutex::default(),
        })
    }

    async fn run(self: Arc<Self>, mut pushes: mpsc::UnboundedReceiver<ObservedPush>) {
        self.resume_known().await;
        loop {
            tokio::select! {
                () = self.quit.cancelled() => break,
                push = pushes.recv() => match push {
                    Some(push) => self.take(push).await,
                    None => break,
                },
            }
        }
    }

    /// Open the queues the channels' projects have, recovering them.
    async fn resume_known(self: &Arc<Self>) {
        for folder in project_folders(&self.hub).await {
            let Ok(repo) = RepoPaths::of(&folder).await else {
                continue;
            };
            if has_queue_file(&repo).await {
                if let Err(why) = self.queue_for(repo).await {
                    warn!(folder = %folder.display(), %why, "Could not resume a merge queue");
                }
            }
        }
    }

    /// Send channel `channel_id`'s QUEUE tab its repo's queue, opening the
    /// queue when the repo has one on disk.
    pub(crate) async fn show(self: &Arc<Self>, channel_id: &str) {
        let folders = channel_folders(&self.hub).await;
        let Some((_, folder)) = folders.into_iter().find(|(id, _)| id == channel_id) else {
            return;
        };
        let Ok(repo) = RepoPaths::of(&folder).await else {
            return;
        };
        let known = self.queues.lock().await.get(&repo.common).cloned();
        let repo_queue = match known {
            Some(found) => found,
            None if has_queue_file(&repo).await => match self.queue_for(repo).await {
                Ok(opened) => opened,
                Err(why) => {
                    warn!(%channel_id, %why, "Could not open a channel's merge queue");
                    return;
                }
            },
            None => return,
        };
        self.publish(&repo_queue).await;
    }

    /// Queue `observed` in the queue of the repo its agent works in.
    async fn take(self: &Arc<Self>, observed: ObservedPush) {
        let ObservedPush { agent, push } = observed;
        let ui = AgentUi::new(self.commands.clone(), agent);
        let Some(folder) = lock(&self.hub.registry).project_folder(agent) else {
            warn!(%agent, branch = %push.branch, "A push from an agent with no folder; not queued");
            return;
        };
        let queued = async {
            let repo = RepoPaths::of(&folder).await.map_err(|e| e.to_string())?;
            let repo_queue = self.queue_for(repo).await?;
            let request = MergeRequest {
                push,
                origin: origin_of(&self.hub, agent),
            };
            let branch = request.push.branch.clone();
            let enqueued = repo_queue
                .queue
                .enqueue(request)
                .await
                .map_err(|e| e.to_string())?;
            if !matches!(enqueued, Enqueued::Duplicate(_)) {
                self.scan_overlaps(&repo_queue, enqueued.id());
            }
            Ok::<_, String>(queued_note(&branch, &repo_queue.target, &enqueued))
        };
        let note = queued.await.unwrap_or_else(|why| {
            warn!(%agent, %why, "Could not queue a push");
            Some(format!("Merge queue: could not queue the push: {why}"))
        });
        if let Some(note) = note {
            info!(%agent, %note, "Push queued");
            ui.send(system_message(note)).await;
        }
    }

    /// Record the files entry `id` changes and warn the owners of work it
    /// overlaps, off this task ([`super::overlap`]).
    fn scan_overlaps(&self, repo_queue: &RepoQueue, id: &aura_exec_merge_queue::EntryId) {
        let (hub, queue) = (Arc::clone(&self.hub), repo_queue.queue.clone());
        let (target, id) = (repo_queue.target.clone(), id.clone());
        tokio::spawn(async move { super::overlap::scan(&hub, &queue, &target, &id).await });
    }

    /// The queue of `repo`, opened (and its processor started) the first
    /// time.
    async fn queue_for(self: &Arc<Self>, repo: RepoPaths) -> Result<Arc<RepoQueue>, String> {
        let mut queues = self.queues.lock().await;
        if let Some(found) = queues.get(&repo.common) {
            return Ok(Arc::clone(found));
        }
        // The target as committed on main, so every worktree agrees; the
        // checks come from each entry's target commit, not this folder.
        let target = aura_exec_merge_queue::resolve_target(&repo.top).await;
        let config = QueueConfig::unchecked(&target);
        let mut integrator = GitIntegrator::new(repo.clone(), config.clone());
        if let Some(forge) = forge_of(&repo.top).await {
            integrator = integrator.with_forge(forge);
        }
        let integrator = Arc::new(integrator);
        let queue = MergeQueue::open(repo.clone(), integrator, Limits::default())
            .await
            .map_err(|e| e.to_string())?;
        info!(repo = %repo.top.display(), target = %config.target, "Merge queue opened");
        let repo_queue = Arc::new(RepoQueue {
            queue,
            target: config.target,
        });
        queues.insert(repo.common.clone(), Arc::clone(&repo_queue));
        drop(queues);
        // Recovery's failures come back as events once it holds the lock.
        self.spawn_processor(&repo_queue);
        self.publish(&repo_queue).await;
        Ok(repo_queue)
    }

    fn spawn_processor(self: &Arc<Self>, repo_queue: &Arc<RepoQueue>) {
        let queue = repo_queue.queue.clone();
        let events = queue.subscribe();
        let cancel = self.quit.child_token();
        tokio::spawn(async move { queue.run(cancel).await });
        tokio::spawn(Arc::clone(self).watch(Arc::clone(repo_queue), events));
    }

    /// Follow `repo_queue`'s changes into the QUEUE tabs and back to the
    /// agents whose merges ended.
    async fn watch(
        self: Arc<Self>,
        repo_queue: Arc<RepoQueue>,
        mut events: broadcast::Receiver<QueueEvent>,
    ) {
        loop {
            let event = tokio::select! {
                () = self.quit.cancelled() => return,
                event = events.recv() => event,
            };
            match event {
                Ok(QueueEvent::Finished(entry)) => {
                    self.send_back(&entry, &repo_queue.target).await;
                }
                Ok(QueueEvent::Held { entry, hold }) => {
                    self.tell_held(&entry, &hold).await;
                    if hold.kind == HoldKind::TargetBroken {
                        self.report_broken(&repo_queue, &hold).await;
                    }
                }
                Ok(QueueEvent::Changed) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            }
            self.publish(&repo_queue).await;
        }
    }

    /// Tell the agent that pushed `entry` how its merge ended: a line in
    /// its chat when it merged, a notice it acts on when it failed.
    async fn send_back(&self, entry: &MergeEntry, target: &str) {
        let Some(agent) = entry
            .origin
            .agent
            .as_deref()
            .and_then(|hex| AgentId::from_hex(hex).ok())
        else {
            return;
        };
        let line = outcome_line(entry);
        if entry.status == MergeStatus::Failed {
            let notice = Notice {
                to: agent,
                shown: line.clone(),
                prompt: failure_prompt(entry, target),
                inbox: None,
            };
            if self.hub.notices.send(notice).is_err() {
                warn!(%agent, "The loop is gone; a merge failure was not delivered");
            }
        } else {
            AgentUi::new(self.commands.clone(), agent)
                .send(system_message(line))
                .await;
        }
    }

    /// Tell the agent whose entry ran into `hold` that it waits, through
    /// no fault of its own.
    async fn tell_held(&self, entry: &MergeEntry, hold: &Hold) {
        let agent = entry.origin.agent.as_deref();
        let Some(agent) = agent.and_then(|hex| AgentId::from_hex(hex).ok()) else {
            return;
        };
        AgentUi::new(self.commands.clone(), agent)
            .send(system_message(held_line(entry, hold)))
            .await;
    }

    /// Report the broken target `hold` names, once per tip: to the
    /// coordinator of each channel on the repo, and to the agents whose
    /// queue entries merged a suspect commit.
    async fn report_broken(&self, repo_queue: &RepoQueue, hold: &Hold) {
        let tip = hold.target_tip.clone().unwrap_or_default();
        let common = repo_queue.queue.repo().common.display().to_string();
        let first = lock(&self.reported).insert(format!("{common}@{tip}"));
        if !first {
            return;
        }
        let entries = repo_queue.queue.entries().await;
        let merged_by = |sha: &str| {
            entries
                .iter()
                .find(|entry| entry.merged.as_deref() == Some(sha))
                .map(|entry| entry.origin.clone())
        };
        let origins: Vec<_> = hold.suspects.iter().map(|s| merged_by(&s.sha)).collect();
        let named = named_suspects(hold, &origins);
        let target = &repo_queue.target;
        let shown = format!("Merge queue: {target} is broken; {}", hold.reason);
        warn!(%target, %tip, "The merge queue found the target broken");
        let coordinators = coordinators_on(&self.hub, &repo_queue.queue.repo().common).await;
        let suspects = origins
            .iter()
            .flatten()
            .filter_map(|origin| origin.agent.as_deref())
            .filter_map(|hex| AgentId::from_hex(hex).ok());
        let mut told = Vec::new();
        for (to, to_suspect) in coordinators
            .into_iter()
            .map(|id| (id, false))
            .chain(suspects.map(|id| (id, true)))
        {
            if told.contains(&to) {
                continue;
            }
            told.push(to);
            let notice = Notice {
                to,
                shown: shown.clone(),
                prompt: broken_target_prompt(target, hold, &named, to_suspect),
                inbox: None,
            };
            if self.hub.notices.send(notice).is_err() {
                warn!(%to, "The loop is gone; a broken target was not reported");
            }
        }
    }

    /// Send `repo_queue` to the QUEUE tab of every channel on its repo,
    /// each as much of its history as the tab shows.
    async fn publish(&self, repo_queue: &RepoQueue) {
        let queue = &repo_queue.queue;
        let channels = channels_on(&self.hub, &queue.repo().common).await;
        let views = pages::views(queue, &repo_queue.target, channels, &self.windows).await;
        for (channel_id, view) in views {
            let command = UiCommand::SetMergeQueue {
                channel_id,
                queue: view,
            };
            if self.commands.send(command).await.is_err() {
                // The UI closed: no tab is left to show it to.
                return;
            }
        }
    }
}

/// `hold`'s suspects, each with the agent whose entry merged it
/// (`origins`, in the same order).
fn named_suspects<'a>(hold: &'a Hold, origins: &[Option<Origin>]) -> Vec<Named<'a>> {
    hold.suspects
        .iter()
        .zip(origins)
        .map(|(suspect, origin)| Named {
            suspect,
            agent: origin.as_ref().map(|o| {
                o.agent_name
                    .clone()
                    .unwrap_or_else(|| "an agent".to_string())
            }),
        })
        .collect()
}

/// Who `agent` is, for its queue entries.
fn origin_of(hub: &AgentHub, agent: AgentId) -> Origin {
    let agent_name = lock(&hub.registry)
        .find(agent)
        .map(|found| found.name.clone());
    let channel = hub
        .channels
        .as_ref()
        .and_then(|store| store.channel_of(&agent).ok().flatten());
    Origin {
        agent: Some(agent.to_hex()),
        agent_name,
        channel,
    }
}
