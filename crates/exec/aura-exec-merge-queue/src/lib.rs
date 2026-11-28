//! The software factory's merge queue.
//!
//! Every push an aura agent makes lands in its repo's queue
//! ([`QueuedPush`] is the marker the push tools put in their results).
//! The queue lands entries on the target branch one at a time: it
//! rebases each onto the latest target in a worktree of its own, runs the
//! repo's build and test there, and fast-forwards the target only when
//! they pass. A failed entry keeps its reason for the agent that pushed
//! it. The queue lives in the repo's common git dir
//! (`<git-common-dir>/aura/merge-queue.json`), so every worktree of a
//! repo shares one queue and it survives restarts.
//!
//! See `docs/software-factory.md` for the whole flow.
//!
//! - [`entry`]: what an entry is, and the push marker.
//! - [`state`]: the queue and every change to it, pure.
//! - [`store`]: the queue file and the one-processor-per-repo lock.
//! - [`lock`]: the operating system's file locks both are held with.
//! - [`config`]: target branch and checks.
//! - [`integrate`]: landing one entry with git.
//! - [`resolve`]: settling a merge that conflicted only in generated files.
//! - [`changelog`]: changelog fragments, assembled at release time.
//! - [`overlap`]: the files the work in flight changes, and where it overlaps.
//! - [`processor`]: the queue at work.

pub mod changelog;
pub mod checks;
pub mod config;
pub mod entry;
pub mod error;
pub mod git;
pub mod integrate;
pub mod lock;
pub mod overlap;
pub mod processor;
pub mod resolve;
pub mod state;
pub mod store;

pub use config::QueueConfig;
pub use entry::{
    short_sha, EntryId, MergeEntry, MergeRequest, MergeStatus, Origin, QueuedPush, MERGE_QUEUE_KEY,
};
pub use error::QueueError;
pub use integrate::{GitIntegrator, Integrator, RepoPaths};
pub use processor::{Limits, MergeQueue, QueueEvent};
pub use state::{Enqueued, QueueState, RunOutcome};
