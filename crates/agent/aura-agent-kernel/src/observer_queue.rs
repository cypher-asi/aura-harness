//! Record observers that run off the kernel's commit path.
//!
//! [`RecordObserver::on_entry`] runs on the committing kernel's task. An
//! observer that formats rows for a UI or appends to a file makes every
//! agent's commit wait for it; with many agents those waits add up on the
//! runtime's workers. [`QueuedObserver`] hands each entry to a thread of
//! its own instead.

use crate::RecordObserver;
use aura_core_types::RecordEntry;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::thread::ThreadId;

/// Entries waiting for the observer thread before committing kernels
/// wait for it.
const QUEUE_ENTRIES: usize = 8192;

/// Runs an observer on a thread of its own, in commit order.
///
/// A full queue holds the committing kernel until the thread catches up,
/// so no entry is lost. When no thread can be started the observer runs
/// inline, as it would unqueued.
#[derive(Debug)]
pub struct QueuedObserver {
    to: Route,
}

#[derive(Debug)]
enum Route {
    Queue {
        items: SyncSender<Item>,
        thread: ThreadId,
    },
    Inline(Arc<dyn RecordObserver>),
}

#[derive(Debug)]
enum Item {
    Entry(Box<RecordEntry>),
    Flushed(SyncSender<()>),
}

impl QueuedObserver {
    /// Hand every entry to `observer` on a thread named `name`, which
    /// ends once this is dropped and the queue is drained.
    #[must_use]
    pub fn new(name: &str, observer: Arc<dyn RecordObserver>) -> Self {
        let (items, queue) = sync_channel::<Item>(QUEUE_ENTRIES);
        let running = Arc::clone(&observer);
        let started = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                for item in queue {
                    match item {
                        Item::Entry(entry) => running.on_entry(&entry),
                        Item::Flushed(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            });
        let to = match started {
            Ok(handle) => Route::Queue {
                items,
                thread: handle.thread().id(),
            },
            Err(error) => {
                tracing::warn!(%error, "record observer runs on the commit path");
                Route::Inline(observer)
            }
        };
        Self { to }
    }

    /// Wait until the observer has seen every entry handed in so far.
    pub fn flush(&self) {
        let Route::Queue { items, thread } = &self.to else {
            return;
        };
        if std::thread::current().id() == *thread {
            return;
        }
        let (done, flushed) = sync_channel(1);
        if items.send(Item::Flushed(done)).is_ok() {
            let _ = flushed.recv();
        }
    }
}

impl RecordObserver for QueuedObserver {
    fn on_entry(&self, entry: &RecordEntry) {
        match &self.to {
            Route::Queue { items, .. } => {
                let _ = items.send(Item::Entry(Box::new(entry.clone())));
            }
            Route::Inline(observer) => observer.on_entry(entry),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::{AgentId, Transaction};
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct Seen(Mutex<Vec<u64>>);

    impl RecordObserver for Seen {
        fn on_entry(&self, entry: &RecordEntry) {
            self.0.lock().unwrap().push(entry.seq);
        }
    }

    #[test]
    fn entries_reach_the_observer_in_commit_order() {
        let seen = Arc::new(Seen::default());
        let queued = QueuedObserver::new("test-observer", Arc::clone(&seen) as _);
        let agent = AgentId::generate();
        let entries = QUEUE_ENTRIES as u64 * 2;
        for seq in 1..=entries {
            let tx = Transaction::user_prompt(agent, "hi");
            queued.on_entry(&RecordEntry::builder(seq, tx).build());
        }
        queued.flush();
        assert_eq!(*seen.0.lock().unwrap(), (1..=entries).collect::<Vec<_>>());
    }
}
