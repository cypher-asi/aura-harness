//! A snapshot of the inbox and of the loop's part (queues and turns
//! under way), and putting one back after a restart.

use super::file::{
    SavedFlow, SavedKind, SavedNotice, SavedOwed, SavedQueued, SavedRunning, SavedThreads, Snapshot,
};
use super::queue_items::work_of;
use super::*;

/// What the loop holds that the file keeps, as of the loop's last save:
/// tools that file mail off the loop save it with this ([`persist`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct LoopPart {
    pub(super) queued: Vec<SavedQueued>,
    pub(super) running: Vec<SavedRunning>,
    /// The software factory's stages
    pub(super) flows: Vec<SavedFlow>,
}

/// What the file keeps of `inbox`, its outbox and `part`, in a stable
/// order and within its limits.
pub(super) fn build(inbox: &Inbox, part: &LoopPart) -> Snapshot {
    let state = lock(&inbox.state);
    let mut owed: Vec<(&AgentId, &Vec<Owed>)> = state.owed.iter().collect();
    owed.sort_by_cached_key(|(agent, _)| agent.to_hex());
    let owed = owed
        .into_iter()
        .flat_map(|(agent, list)| list.iter().map(|owed| saved_owed(*agent, owed)))
        .collect();
    let threads = saved_threads(&state);
    let mut parked: Vec<String> = state.parked.iter().map(AgentId::to_hex).collect();
    parked.sort();
    drop(state);
    Snapshot {
        owed,
        queued: part.queued.clone(),
        outbox: lock(&inbox.saved.outbox).clone(),
        running: part.running.clone(),
        threads,
        parked,
        flows: part.flows.clone(),
        ..Snapshot::default()
    }
    .capped()
}

/// The threads of `state`, sorted.
fn saved_threads(state: &State) -> SavedThreads {
    let mut requests: Vec<(String, u32)> = state
        .requests
        .iter()
        .map(|(thread, carried)| (thread.clone(), *carried))
        .collect();
    requests.sort();
    let mut subjects: Vec<(String, String, String)> = state
        .subjects
        .iter()
        .map(|((low, high), subject)| (low.to_hex(), high.to_hex(), subject.clone()))
        .collect();
    subjects.sort();
    let mut reports: Vec<(String, bool, String)> = state
        .reports
        .iter()
        .map(|(agent, (ok, said))| (agent.to_hex(), *ok, said.clone()))
        .collect();
    reports.sort();
    SavedThreads {
        requests,
        subjects,
        reports,
    }
}

/// What a restore puts back, its agents known to be live.
#[derive(Debug, Default)]
pub(super) struct Restored {
    pub(super) owed: Vec<(AgentId, Owed)>,
    pub(super) queued: Vec<(AgentId, SavedQueued)>,
    pub(super) outbox: Vec<Notice>,
    pub(super) running: Vec<(AgentId, SavedRunning)>,
    pub(super) requests: Vec<(String, u32)>,
    pub(super) subjects: Vec<((AgentId, AgentId), String)>,
    pub(super) reports: Vec<(AgentId, (bool, String))>,
    pub(super) parked: Vec<AgentId>,
    /// The software factory's stages, put back once the queues are
    pub(super) flows: Vec<SavedFlow>,
}

/// The entries of `snapshot` whose agents are live on `hub`.
pub(super) fn live_entries(hub: &AgentHub, snapshot: Snapshot) -> Restored {
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
        .filter_map(|saved| Some((live(&saved.agent)?, saved)));
    let outbox = snapshot.outbox.into_iter().filter_map(|saved| {
        Some(Notice {
            to: live(&saved.to)?,
            shown: saved.shown,
            prompt: saved.prompt,
            inbox: Some(kind_of(saved.kind)),
        })
    });
    let running = snapshot
        .running
        .into_iter()
        .filter_map(|saved| Some((live(&saved.agent)?, saved)));
    let mut restored = Restored {
        owed: owed.collect(),
        queued: queued.collect(),
        outbox: outbox.collect(),
        running: running.collect(),
        parked: snapshot.parked.iter().filter_map(|hex| live(hex)).collect(),
        flows: snapshot.flows,
        ..Restored::default()
    };
    live_threads(&mut restored, snapshot.threads, &live);
    restored
}

/// Put the threads of `saved` whose agents `live` accepts in `restored`.
fn live_threads(
    restored: &mut Restored,
    saved: SavedThreads,
    live: &dyn Fn(&str) -> Option<AgentId>,
) {
    restored.requests = saved.requests;
    restored.subjects = saved
        .subjects
        .into_iter()
        .filter_map(|(low, high, subject)| Some(((live(&low)?, live(&high)?), subject)))
        .collect();
    restored.reports = saved
        .reports
        .into_iter()
        .filter_map(|(agent, ok, said)| Some((live(&agent)?, (ok, said))))
        .collect();
}

/// What [`put_back`] leaves for the resume step.
#[derive(Debug, Default)]
pub(in crate::event_loop) struct Resume {
    /// Agents that got queued work back
    pub(super) queued: Vec<AgentId>,
    /// Turns under way at the last save
    pub(super) running: Vec<(AgentId, SavedRunning)>,
    /// Queued messages that came back without images, how many each
    pub(super) images_dropped: Vec<(AgentId, usize)>,
}

#[cfg(test)]
impl Resume {
    /// The agents that got queued work back.
    pub(crate) fn queued_agents(&self) -> &[AgentId] {
        &self.queued
    }
}

/// Put `restored` back on the loop of `state`: the inbox's state, the
/// queues, and the undelivered mail, sent again.
pub(super) fn put_back(state: &mut LoopState<'_>, restored: Restored) -> Resume {
    let Restored {
        owed,
        queued,
        outbox,
        running,
        requests,
        subjects,
        reports,
        parked,
        flows,
    } = restored;
    let counts = (owed.len(), queued.len(), outbox.len(), running.len());
    merge_owed(&state.hub.inbox, owed);
    put_threads(&state.hub.inbox, (requests, subjects, reports), parked);
    let mut resume = Resume {
        running,
        ..Resume::default()
    };
    requeue(state, queued, &mut resume);
    // After the queues: a planning turn the inbox queued again isn't
    // queued twice.
    let hub = Arc::clone(&state.hub);
    let resumed =
        crate::event_loop::factory::saved::put_back(state, flows, |hex| live_agent(&hub, hex));
    for agent in resumed {
        if !resume.queued.contains(&agent) {
            resume.queued.push(agent);
        }
    }
    for notice in outbox {
        if super::send(&state.hub, notice).is_err() {
            warn!("A saved inbox message could not be sent again: the terminal is shutting down");
        }
    }
    if counts != (0, 0, 0, 0) {
        info!(
            owed = counts.0,
            queued = counts.1,
            undelivered = counts.2,
            running = counts.3,
            "Restored the saved inbox"
        );
    }
    resume
}

/// What threads carried: requests by thread, subjects by pair, reports.
type Threads = (
    Vec<(String, u32)>,
    Vec<((AgentId, AgentId), String)>,
    Vec<(AgentId, (bool, String))>,
);

/// Put `threads` and `parked` back in `inbox`, keeping what it has.
fn put_threads(inbox: &Inbox, (requests, subjects, reports): Threads, parked: Vec<AgentId>) {
    let mut inbox = lock(&inbox.state);
    for (thread, carried) in requests {
        inbox.requests.entry(thread).or_insert(carried);
    }
    for (pair, subject) in subjects {
        inbox.subjects.entry(pair).or_insert(subject);
    }
    for (agent, report) in reports {
        inbox.reports.entry(agent).or_insert(report);
    }
    inbox.parked.extend(parked);
}

/// `hex` as the id of an agent live on `hub`.
fn live_agent(hub: &AgentHub, hex: &str) -> Option<AgentId> {
    let registry = lock(&hub.registry);
    AgentId::from_hex(hex).ok().filter(|id| {
        registry
            .find(*id)
            .is_some_and(|a| !a.archived && !a.removed)
    })
}

/// Queue `queued` again, noting in `resume` who got work back and which
/// messages came back without images.
fn requeue(state: &mut LoopState<'_>, queued: Vec<(AgentId, SavedQueued)>, resume: &mut Resume) {
    for (agent, saved) in queued {
        if saved.images_dropped > 0 {
            resume.images_dropped.push((agent, saved.images_dropped));
        }
        state.queued.push_back(agent, work_of(saved));
        if !resume.queued.contains(&agent) {
            resume.queued.push(agent);
        }
    }
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

/// `notice`, an inbox message, as the outbox keeps it.
pub(super) fn saved_notice(notice: &Notice, kind: InboxKind) -> SavedNotice {
    SavedNotice {
        to: notice.to.to_hex(),
        shown: notice.shown.clone(),
        prompt: notice.prompt.clone(),
        kind: saved_kind(kind),
    }
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
        taken: owed.taken,
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
        taken: saved.taken,
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
