//! The software factory's merge queue.
//!
//! Every push an aura agent makes lands in its repo's queue
//! ([`QueuedPush`] is the marker the push tools put in their results).
//! The queue lands entries on the target branch one at a time: it
//! rebases each onto the latest target in a worktree of its own, runs the
//! repo's build and test there, and fast-forwards the target only when
//! they pass. A failed entry keeps its reason for the agent that pushed
//! it. The queue lives in the repo's common git dir
//! (`<git-common-dir>/aura/merge-queue.json`, with its full history in
//! `merge-history.jsonl` next to it), so every worktree of a repo shares
//! one queue and it survives restarts. Its worktree and build output live
//! outside `.git`, in a work folder under the user's cache folder
//! ([`RepoPaths`]).
//!
//! See `docs/software-factory.md` for the whole flow.
//!
//! - [`entry`]: what an entry is, and the push marker.
//! - [`state`]: the queue and every change to it, pure.
//! - [`store`]: the queue file and the one-processor-per-repo lock.
//! - [`history`]: every finished entry the queue file no longer keeps.
//! - [`lock`]: the operating system's file locks both are held with.
//! - [`config`]: the checks, and the config file's target.
//! - [`target`]: the target branch, as committed on `main`.
//! - [`integrate`]: landing one entry with git.
//! - [`resolve`]: settling a merge that conflicted only in generated files.
//! - [`changelog`]: changelog fragments, assembled at release time.
//! - [`overlap`]: the files the work in flight changes, and where it overlaps.
//! - [`processor`]: the queue at work.

pub mod changelog;
mod checks;
mod config;
mod entry;
mod error;
mod git;
mod history;
mod integrate;
mod lock;
pub mod overlap;
mod processor;
mod repo;
mod resolve;
mod state;
mod store;
mod target;

pub use config::{target_in, QueueConfig, CONFIG_PATH, DEFAULT_TARGET, GIT_TIMEOUT};
pub use entry::{
    short_sha, EntryId, MergeEntry, MergeRequest, MergeStatus, Origin, QueuedPush, MERGE_QUEUE_KEY,
};
pub use error::QueueError;
pub use integrate::{GitIntegrator, Integrator, RepoPaths};
pub use lock::FileLock;
pub use processor::{Limits, MergeQueue, Processor, QueueEvent, QueuePage};
pub use state::{Enqueued, HistoryMark, QueueState, RunOutcome};
pub use store::QUEUE_FILE;
pub use target::resolve_target;
