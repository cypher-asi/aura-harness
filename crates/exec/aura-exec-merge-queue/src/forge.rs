//! The forge a repo's PRs live on (GitHub), as the queue uses it: find
//! or open a branch's PR, comment on it, set the queue's commit status,
//! and merge it pinned to the head the queue tested.
//!
//! ## Invariants
//!
//! - [`Forge::merge`] merges only when the PR's head is still the commit
//!   it is given: a head that moved is [`MergeOutcome::HeadMoved`], never
//!   a merge of something untested.
//! - Every call has a time limit ([`FORGE_TIMEOUT`] for [`GhForge`]).
//! - A forge that can't be reached or refuses the credentials says so
//!   with [`ForgeError::Unavailable`], which holds the queue rather than
//!   failing an entry that did nothing wrong.

use crate::report::PrRef;
use async_trait::async_trait;
use std::time::Duration;

mod gh;

pub use gh::{GhForge, RepoSlug};

/// How long one forge call may take.
pub const FORGE_TIMEOUT: Duration = Duration::from_secs(60);

/// The commit status context the queue sets on every head it tests.
/// Branch protection on the target that requires it keeps a hand merge
/// from skipping the queue's checks.
pub const STATUS_CONTEXT: &str = "aura/merge-queue";

/// Where a PR is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    /// Open, not merged
    Open,
    /// Closed without a merge: abandoned
    Closed,
    /// Merged
    Merged,
}

/// A PR as the forge has it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    /// Its number and page
    pub pr: PrRef,
    /// Where it is in its life
    pub state: PrState,
    /// The commit its branch points at
    pub head: String,
    /// The commit its merge made, once merged
    pub merge_commit: Option<String>,
}

/// The PR a branch should land through.
#[derive(Debug, Clone, Copy)]
pub struct PrRequest<'a> {
    /// The branch it merges
    pub branch: &'a str,
    /// The branch it merges into
    pub target: &'a str,
    /// Its title, when the queue opens it
    pub title: &'a str,
    /// Its description, when the queue opens it
    pub body: &'a str,
}

/// A commit status's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    /// The queue is testing it
    Pending,
    /// Its merge passed the build and tests
    Success,
    /// It conflicted or failed a check
    Failure,
}

impl CheckState {
    /// The name the forge's API uses.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }
}

/// How a merge went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// Merged; the target holds `sha`, the forge's merge commit
    Merged {
        /// The merge commit
        sha: String,
    },
    /// The PR's head is no longer the tested one; it is `now`
    HeadMoved {
        /// The head the PR has now
        now: String,
    },
    /// The forge would not merge it, and why
    Refused(String),
}

/// Why a forge call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ForgeError {
    /// The forge can't be reached, the CLI is missing, or it refused the
    /// credentials: not the entry's fault
    #[error("GitHub is unavailable: {0}")]
    Unavailable(String),
    /// The call reached the forge and failed
    #[error("GitHub: {0}")]
    Failed(String),
}

/// A forge the queue lands PRs through.
#[async_trait]
pub trait Forge: Send + Sync {
    /// The open PR of `request.branch` into `request.target`, opened when
    /// there is none.
    async fn open_or_find(&self, request: &PrRequest<'_>) -> Result<PullRequest, ForgeError>;

    /// PR `number` as it is now.
    async fn pr(&self, number: u64) -> Result<PullRequest, ForgeError>;

    /// Comment `body` on PR `number`.
    async fn comment(&self, number: u64, body: &str) -> Result<(), ForgeError>;

    /// Set the queue's status ([`STATUS_CONTEXT`]) on commit `sha`.
    async fn set_status(&self, sha: &str, state: CheckState, text: &str) -> Result<(), ForgeError>;

    /// Merge PR `number` with a merge commit, only while its head is
    /// `head`.
    async fn merge(&self, number: u64, head: &str) -> Result<MergeOutcome, ForgeError>;
}
