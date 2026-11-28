//! What agents owe and what they were doing, kept between runs in
//! [`SAVED_FILE`] in the app's data directory: the reports owed, every
//! agent's queued work, the inbox mail not yet delivered (the outbox),
//! the turns under way, the inbox's threads and the agents the user
//! parked.
//!
//! The event loop restores it as it starts ([`restore_saved`]), then
//! resumes what the restart cut short ([`resume_saved`]): members carry
//! on on their own, cloud turns are followed again on their machines,
//! and the reports go back to whoever assigned the work, with the todo
//! each closes. It saves after every event that changed it, waiting for
//! the write ([`save_now`]), and the tools that file mail save before
//! they tell the sender it went ([`persist`]). So a crash loses nothing
//! a sender was told about.
//!
//! # Invariants
//!
//! * The file is read and written on blocking threads, never on the
//!   async runtime's.
//! * Writes are numbered under one lock as their snapshot is taken: an
//!   older snapshot never replaces a newer one.
//! * Entries for agents that are gone, archived or removed are dropped
//!   on restore.
//! * A hub with no file path (most tests) keeps nothing.
//!
//! # Failure modes
//!
//! See [`file`]: a bad file is an empty inbox, with a warning. A failed
//! write is logged and retried at the next change; the tool that asked
//! for it still succeeds.

use super::*;
use file::{SavedNotice, SavedRunning, Snapshot};
use snapshot::LoopPart;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

mod file;
mod queue_items;
mod resume;
mod snapshot;
#[cfg(test)]
mod tests;

pub(crate) use file::SAVED_FILE;
pub(in crate::event_loop) use resume::{resume_prompt_for, resume_saved};
pub(in crate::event_loop) use snapshot::Resume;

/// Where the inbox is kept, what was last written there, and what is
/// kept only for the file.
#[derive(Default)]
pub(crate) struct Keeper {
    /// The file; unset until the loop restores it
    path: OnceLock<PathBuf>,
    /// The last snapshot handed to a write; held while one is taken
    last: Mutex<Option<Snapshot>>,
    /// Number of the last write handed out
    handed: AtomicU64,
    /// Number of the last write done; held while writing
    written: Mutex<u64>,
    /// Inbox mail sent and not yet given to its agent, oldest first
    outbox: Mutex<Vec<SavedNotice>>,
    /// The loop's part as of its last save
    loop_part: Mutex<LoopPart>,
}

impl Keeper {
    /// The next write's number.
    fn next_write(&self) -> u64 {
        self.handed.fetch_add(1, Ordering::Relaxed) + 1
    }
}

/// Send `notice`, an inbox message, to the loop, keeping it in the
/// outbox until the loop gives it to its agent ([`delivered`]).
///
/// # Errors
/// When the loop is gone: the terminal is shutting down.
pub(super) fn send(hub: &AgentHub, notice: Notice) -> Result<(), ()> {
    if let Some(kind) = notice.inbox {
        lock(&hub.inbox.saved.outbox).push(snapshot::saved_notice(&notice, kind));
    }
    hub.notices.send(notice).map_err(|_| ())
}

/// The loop is giving `notice` to its agent: it leaves the outbox.
pub(in crate::event_loop) fn delivered(hub: &AgentHub, to: AgentId, prompt: &str) {
    let to = to.to_hex();
    let mut outbox = lock(&hub.inbox.saved.outbox);
    if let Some(at) = outbox
        .iter()
        .position(|saved| saved.to == to && saved.prompt == prompt)
    {
        outbox.remove(at);
    }
}

/// As the loop starts: read the saved file and put it back. Owed
/// reports go ahead of any the agent owes already, queued work to the
/// back of its agent's queue, and undelivered mail is sent again. Starts
/// nothing: [`resume_saved`] does, with what this returns.
pub(in crate::event_loop) async fn restore_saved(state: &mut LoopState<'_>) -> Resume {
    let path = lock(&state.hub.registry).data_dir().join(SAVED_FILE);
    let keeper = &state.hub.inbox.saved;
    let path = keeper.path.get_or_init(|| path).clone();
    let read = tokio::task::spawn_blocking(move || file::load(&path)).await;
    let snapshot = read.unwrap_or_else(|e| {
        warn!(error = %e, "Reading the saved inbox failed; starting with an empty one");
        Snapshot::default()
    });
    // Until the loop saves its own part, saves off the loop keep these.
    *lock(&keeper.loop_part) = LoopPart {
        queued: snapshot.queued.clone(),
        running: snapshot.running.clone(),
        flows: snapshot.flows.clone(),
    };
    let restored = snapshot::live_entries(&state.hub, snapshot);
    snapshot::put_back(state, restored)
}

/// Save if anything changed since the last save, and wait for the
/// write. The loop runs it after every event.
pub(in crate::event_loop) async fn save_now(state: &LoopState<'_>) {
    if let Some(write) = keep_saved(state) {
        if let Err(e) = write.await {
            warn!(error = %e, "Saving the inbox did not finish");
        }
    }
}

/// [`save_now`] without waiting: the write's handle, `None` when
/// nothing changed.
pub(in crate::event_loop) fn keep_saved(
    state: &LoopState<'_>,
) -> Option<tokio::task::JoinHandle<()>> {
    state.hub.inbox.saved.path.get()?;
    let part = loop_part(state);
    let (number, snapshot) = take(&state.hub, Some(part))?;
    let hub = Arc::clone(&state.hub);
    Some(tokio::task::spawn_blocking(move || {
        write(&hub.inbox.saved, number, &snapshot);
    }))
}

/// As the loop shuts down, before it stops what runs: save, with the
/// turns still under way, and wait for the write.
pub(in crate::event_loop) async fn flush_saved(state: &LoopState<'_>) {
    save_now(state).await;
}

/// Off the loop, once mail was filed: save the inbox with the loop's
/// part as of its last save, and wait for the write, so the sender is
/// told the mail went only once it is on disk.
pub(super) async fn persist(hub: &Arc<AgentHub>) {
    let Some((number, snapshot)) = take(hub, None) else {
        return;
    };
    let hub = Arc::clone(hub);
    let wrote = tokio::task::spawn_blocking(move || write(&hub.inbox.saved, number, &snapshot));
    if let Err(e) = wrote.await {
        warn!(error = %e, "Saving the inbox did not finish");
    }
}

/// Take a snapshot with `part`, or the loop's last one, and its write
/// number, unless it is what was last written.
fn take(hub: &AgentHub, part: Option<LoopPart>) -> Option<(u64, Snapshot)> {
    let keeper = &hub.inbox.saved;
    keeper.path.get()?;
    let mut last = lock(&keeper.last);
    let part = match part {
        Some(part) => {
            lock(&keeper.loop_part).clone_from(&part);
            part
        }
        None => lock(&keeper.loop_part).clone(),
    };
    let snapshot = snapshot::build(&hub.inbox, &part);
    if last.as_ref() == Some(&snapshot) {
        return None;
    }
    *last = Some(snapshot.clone());
    Some((keeper.next_write(), snapshot))
}

/// The loop's part now: every queue, and the turns under way here and
/// on cloud machines.
fn loop_part(state: &LoopState<'_>) -> LoopPart {
    let items = state.queued.items();
    let mut running: Vec<SavedRunning> = state
        .hub
        .running_agents()
        .into_iter()
        .map(|agent| SavedRunning {
            agent: agent.to_hex(),
            cloud: false,
        })
        .collect();
    running.extend(
        state
            .cloud
            .cloud_turn_agents()
            .into_iter()
            .map(|agent| SavedRunning {
                agent: agent.to_hex(),
                cloud: true,
            }),
    );
    running.sort_by(|a, b| a.agent.cmp(&b.agent));
    LoopPart {
        queued: queue_items::saved_items(&items),
        running,
        flows: crate::event_loop::factory::saved::saved_flows(state),
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

#[cfg(test)]
impl Inbox {
    /// Forget everything, as a restart does.
    pub(crate) fn forget_all(&self) {
        *lock(&self.state) = State::default();
        *lock(&self.saved.last) = None;
        lock(&self.saved.outbox).clear();
        *lock(&self.saved.loop_part) = LoopPart::default();
    }
}
