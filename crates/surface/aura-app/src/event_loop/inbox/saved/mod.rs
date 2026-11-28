//! The inbox kept between runs: the reports agents owe, the inbox
//! messages waiting in their queues, and the ones sent but not yet given
//! to their agent, in [`SAVED_FILE`] in the app's data directory.
//!
//! The event loop restores them as it starts ([`restore_saved`], then
//! [`resume_saved`]), saves them whenever they change ([`keep_saved`]),
//! and once more as it shuts down ([`flush_saved`]). So after a restart
//! a member's next turn that ends well still reports to whoever assigned
//! the work, with the todo it closes, and the stall sweep sees what is
//! owed. What was settled before the save (answered, given up, dropped
//! as withdrawn) is not in the file, and an entry taken back stays
//! withdrawn.
//!
//! # Invariants
//!
//! * The file is read and written on blocking threads, never on the
//!   async runtime's.
//! * Writes are numbered: an older one never replaces a newer one.
//! * Entries for agents that are gone, archived or removed are dropped
//!   on restore.
//! * A hub with no file path (most tests) keeps nothing.
//!
//! # Failure modes
//!
//! See [`file`]: a bad file is an empty inbox, with a warning. A failed
//! write is logged and retried at the next change.

use super::*;
use crate::event_loop::queue::WorkQueues;
use file::{SavedKind, SavedNotice, SavedOwed, SavedQueued, SavedRequest, Snapshot};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc::UnboundedReceiver;

mod file;
#[cfg(test)]
mod tests;

pub(crate) use file::SAVED_FILE;

/// Where the inbox is kept, and what was last written there.
#[derive(Default)]
pub(crate) struct Keeper {
    /// The file; unset until the loop restores it
    path: OnceLock<PathBuf>,
    /// The last snapshot handed to a write
    last: Mutex<Option<Snapshot>>,
    /// Number of the last write handed out
    handed: AtomicU64,
    /// Number of the last write done; held while writing
    written: Mutex<u64>,
}

impl Keeper {
    /// The next write's number.
    fn next_write(&self) -> u64 {
        self.handed.fetch_add(1, Ordering::Relaxed) + 1
    }
}

/// What a restore puts back, its agents known to be live.
#[derive(Debug, Default)]
struct Restored {
    owed: Vec<(AgentId, Owed)>,
    queued: Vec<(AgentId, String, Request)>,
    notices: Vec<Notice>,
}

/// As the loop starts: read the saved inbox and put it back. Owed
/// reports go ahead of any the agent owes already, queued messages to
/// the back of their agents' queues, and undelivered messages are sent
/// again. Returns the agents that got queued work back, for
/// [`resume_saved`].
pub(in crate::event_loop) async fn restore_saved(state: &mut LoopState<'_>) -> Vec<AgentId> {
    let path = lock(&state.hub.registry).data_dir().join(SAVED_FILE);
    let keeper = &state.hub.inbox.saved;
    let path = keeper.path.get_or_init(|| path).clone();
    let read = tokio::task::spawn_blocking(move || file::load(&path)).await;
    let snapshot = read.unwrap_or_else(|e| {
        warn!(error = %e, "Reading the saved inbox failed; starting with an empty one");
        Snapshot::default()
    });
    let restored = live_entries(&state.hub, snapshot);
    put_back(state, restored)
}

/// The entries of `snapshot` whose agents are live on `hub`.
fn live_entries(hub: &AgentHub, snapshot: Snapshot) -> Restored {
    let registry = lock(&hub.registry);
    let live = |hex: &str| {
        AgentId::from_hex(hex).ok().filter(|id| {
            registry
                .find(*id)
                .is_some_and(|a| !a.archived && !a.removed)
        })
    };
    let owed = snapshot.owed.into_iter().filter_map(|saved| {
        let (agent, from) = (live(&saved.agent)?, live(&saved.from)?);
        Some((agent, owed_of(saved, from)))
    });
    let queued = snapshot
        .queued
        .into_iter()
        .filter_map(|saved| Some((live(&saved.agent)?, saved.text, request_of(saved.request))));
    let notices = snapshot.notices.into_iter().filter_map(|saved| {
        Some(Notice {
            to: live(&saved.to)?,
            shown: saved.shown,
            prompt: saved.prompt,
            inbox: Some(kind_of(saved.kind)),
        })
    });
    Restored {
        owed: owed.collect(),
        queued: queued.collect(),
        notices: notices.collect(),
    }
}

/// Put `restored` back on the loop of `state`. Returns the agents that
/// got queued work back.
fn put_back(state: &mut LoopState<'_>, restored: Restored) -> Vec<AgentId> {
    let Restored {
        owed,
        queued,
        notices,
    } = restored;
    let counts = (owed.len(), queued.len(), notices.len());
    merge_owed(&state.hub.inbox, owed);
    let mut agents = Vec::new();
    for (agent, text, request) in queued {
        let work = Work::Prompt {
            prompt: Prompt::inbox(text, request),
            shown: false,
        };
        state.queued.push_back(agent, work);
        if !agents.contains(&agent) {
            agents.push(agent);
        }
    }
    for notice in notices {
        if state.hub.notices.send(notice).is_err() {
            warn!("A saved inbox message could not be sent again: the terminal is shutting down");
        }
    }
    if counts != (0, 0, 0) {
        info!(
            owed = counts.0,
            queued = counts.1,
            undelivered = counts.2,
            "Restored the saved inbox"
        );
    }
    agents
}

/// Add `restored` to what `inbox` owes, each agent's restored reports
/// ahead of its own, skipping any it owes already.
fn merge_owed(inbox: &Inbox, restored: Vec<(AgentId, Owed)>) {
    let mut state = lock(&inbox.state);
    let mut by_agent: Vec<(AgentId, Vec<Owed>)> = Vec::new();
    for (agent, owed) in restored {
        match by_agent.iter_mut().find(|(id, _)| *id == agent) {
            Some((_, list)) => list.push(owed),
            None => by_agent.push((agent, vec![owed])),
        }
    }
    for (agent, mut list) in by_agent {
        let current = state.owed.remove(&agent).unwrap_or_default();
        list.retain(|owed| !current.iter().any(|known| known.id == owed.id));
        list.extend(current);
        if !list.is_empty() {
            state.owed.insert(agent, list);
        }
    }
}

/// Once the loop has started: start the work restored for `agents`
/// that has nothing running, and tell the placed members their peers
/// now rather than at the first stall sweep.
pub(in crate::event_loop) async fn resume_saved(state: &mut LoopState<'_>, agents: Vec<AgentId>) {
    for agent in agents {
        let busy = state.hub.is_running(agent)
            || state.cloud.is_running(agent)
            || state.cloud.is_moving(agent);
        if !busy {
            crate::event_loop::start_next(state, agent).await;
        }
        crate::event_loop::hud::send_queued(state, agent).await;
    }
    crate::event_loop::cloud::refresh_reach(state).await;
}

/// Save the inbox if it changed since the last save. The write runs on
/// a blocking thread; the loop needn't wait for it, and gets its handle
/// only for tests. `None` when nothing is written.
pub(in crate::event_loop) fn keep_saved(
    state: &LoopState<'_>,
) -> Option<tokio::task::JoinHandle<()>> {
    let keeper = &state.hub.inbox.saved;
    keeper.path.get()?;
    let snapshot = snapshot(&state.hub.inbox, &state.queued, Vec::new());
    {
        let mut last = lock(&keeper.last);
        if last.as_ref() == Some(&snapshot) {
            return None;
        }
        *last = Some(snapshot.clone());
    }
    let number = keeper.next_write();
    let hub = Arc::clone(&state.hub);
    Some(tokio::task::spawn_blocking(move || {
        write(&hub.inbox.saved, number, &snapshot);
    }))
}

/// As the loop shuts down, before its queues are dropped: save the
/// inbox with the inbox messages still in `notice_rx`, and wait for the
/// write. Other notices there are dropped, as they would be anyway.
pub(in crate::event_loop) async fn flush_saved(
    state: &LoopState<'_>,
    notice_rx: &mut UnboundedReceiver<Notice>,
) {
    let keeper = &state.hub.inbox.saved;
    if keeper.path.get().is_none() {
        return;
    }
    let mut undelivered = Vec::new();
    while let Ok(notice) = notice_rx.try_recv() {
        if let Some(kind) = notice.inbox {
            undelivered.push(SavedNotice {
                to: notice.to.to_hex(),
                shown: notice.shown,
                prompt: notice.prompt,
                kind: saved_kind(kind),
            });
        }
    }
    let snapshot = snapshot(&state.hub.inbox, &state.queued, undelivered);
    *lock(&keeper.last) = Some(snapshot.clone());
    let number = keeper.next_write();
    let hub = Arc::clone(&state.hub);
    let wrote = tokio::task::spawn_blocking(move || write(&hub.inbox.saved, number, &snapshot));
    if let Err(e) = wrote.await {
        warn!(error = %e, "The last save of the inbox did not finish");
    }
}

/// Write `snapshot` as write `number`, unless a newer one was written.
fn write(keeper: &Keeper, number: u64, snapshot: &Snapshot) {
    let Some(path) = keeper.path.get() else {
        return;
    };
    let mut written = lock(&keeper.written);
    if *written >= number {
        return;
    }
    match file::save(path, snapshot) {
        Ok(()) => *written = number,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Could not save the inbox; retrying at the next change");
            *lock(&keeper.last) = None;
        }
    }
}

/// What the file keeps of `inbox`, the inbox messages in `queues`, and
/// `notices`, in a stable order and within its limits.
fn snapshot(inbox: &Inbox, queues: &WorkQueues, notices: Vec<SavedNotice>) -> Snapshot {
    let owed: Vec<SavedOwed> = {
        let state = lock(&inbox.state);
        let mut agents: Vec<(&AgentId, &Vec<Owed>)> = state.owed.iter().collect();
        agents.sort_by_cached_key(|(agent, _)| agent.to_hex());
        agents
            .into_iter()
            .flat_map(|(agent, list)| list.iter().map(|owed| saved_owed(*agent, owed)))
            .collect()
    };
    let mut prompts = queues.inbox_prompts();
    prompts.sort_by_cached_key(|(agent, _)| agent.to_hex());
    let queued = prompts
        .into_iter()
        .map(|(agent, prompt)| SavedQueued {
            agent: agent.to_hex(),
            text: prompt.text.clone(),
            request: saved_request(prompt.request),
        })
        .collect();
    Snapshot {
        owed,
        queued,
        notices,
        ..Snapshot::default()
    }
    .capped()
}

fn saved_owed(agent: AgentId, owed: &Owed) -> SavedOwed {
    SavedOwed {
        agent: agent.to_hex(),
        id: owed.id.clone(),
        kind: saved_kind(owed.kind),
        from: owed.from.to_hex(),
        thread: owed.thread.clone(),
        subject: owed.subject.clone(),
        todo: owed.todo.clone(),
        withdrawn: owed.withdrawn,
    }
}

fn owed_of(saved: SavedOwed, from: AgentId) -> Owed {
    Owed {
        id: saved.id,
        kind: kind_of(saved.kind),
        from,
        thread: saved.thread,
        subject: saved.subject,
        todo: saved.todo,
        withdrawn: saved.withdrawn,
    }
}

const fn saved_kind(kind: InboxKind) -> SavedKind {
    match kind {
        InboxKind::Assignment => SavedKind::Assignment,
        InboxKind::Request => SavedKind::Request,
        InboxKind::Reply => SavedKind::Reply,
    }
}

const fn kind_of(kind: SavedKind) -> InboxKind {
    match kind {
        SavedKind::Assignment => InboxKind::Assignment,
        SavedKind::Request => InboxKind::Request,
        SavedKind::Reply => InboxKind::Reply,
    }
}

const fn saved_request(request: Request) -> SavedRequest {
    match request {
        Request::New => SavedRequest::New,
        Request::Build => SavedRequest::Build,
        Request::Resume => SavedRequest::Resume,
    }
}

const fn request_of(request: SavedRequest) -> Request {
    match request {
        SavedRequest::New => Request::New,
        SavedRequest::Build => Request::Build,
        SavedRequest::Resume => Request::Resume,
    }
}

#[cfg(test)]
impl Inbox {
    /// Forget everything, as a restart does.
    pub(crate) fn forget_all(&self) {
        *lock(&self.state) = State::default();
        *lock(&self.saved.last) = None;
    }
}
