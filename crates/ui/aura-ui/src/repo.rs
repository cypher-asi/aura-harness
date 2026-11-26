//! What the REPO side tab shows: the branch the agent's repo is on, its
//! uncommitted changes, a newest-first timeline of what happened in it
//! (commits, pushes, pulls, PRs, merges, resets, checkouts, stashes) and
//! its branches, tags and worktrees.
//!
//! Built by the app ([`UiCommand::SetRepo`](crate::events::UiCommand))
//! from read-only git calls and the GitHub PR list, and drawn by any front
//! end; nothing here reads the repo itself.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The REPO tab of one agent.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RepoView {
    /// Agent whose folder this is (hex id)
    pub agent_id: String,
    /// Top of the repo's working tree
    pub root: PathBuf,
    /// Branch checked out; `None` when `HEAD` is detached
    pub branch: Option<String>,
    /// `HEAD`'s short sha; empty before the first commit
    pub head: String,
    /// The branch's upstream (`origin/main`)
    pub upstream: Option<String>,
    /// Commits on the branch the upstream lacks
    pub ahead: usize,
    /// Commits on the upstream the branch lacks
    pub behind: usize,
    /// Whether this is a linked worktree rather than the main one
    pub worktree: bool,
    /// Uncommitted changes, by path
    pub changes: Vec<RepoChange>,
    /// Uncommitted changes past those in `changes`
    pub changes_more: usize,
    /// What happened, newest first
    pub events: Vec<RepoEvent>,
    /// When the oldest commit read was made, if the commits were cut
    /// there: older events are left out of the mixed timeline
    pub span_start: Option<i64>,
    /// Local branches, the newest commit first
    pub branches: Vec<RepoBranch>,
    /// Tags, the newest first
    pub tags: Vec<RepoTag>,
    /// The repo's worktrees, this one first
    pub worktrees: Vec<RepoWorktree>,
    /// Whether PRs are among the events
    pub prs: PrState,
    /// Why there is nothing to show (`Not a git repository.`)
    pub problem: Option<String>,
}

/// How a file differs from `HEAD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    /// Turned into a link, or back
    TypeChanged,
    /// Not known to git
    Untracked,
    /// Left in conflict by a merge
    Conflicted,
}

impl ChangeKind {
    /// The letter the REPO tab shows for it.
    #[must_use]
    pub const fn letter(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::TypeChanged => "T",
            Self::Untracked => "?",
            Self::Conflicted => "!",
        }
    }
}

/// One uncommitted change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoChange {
    /// Path from the repo's top; an untracked folder ends in `/`
    pub path: String,
    /// Where a renamed file came from
    pub from: Option<String>,
    pub kind: ChangeKind,
    /// Whether any of it is staged
    pub staged: bool,
    /// Whether any of it is not
    pub unstaged: bool,
}

/// What happened in the repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Commit,
    /// A merge commit, or a merge that fast-forwarded
    Merge,
    Rebase,
    Push,
    Pull,
    /// A fetch that moved a remote-tracking branch
    Fetch,
    Reset,
    /// A switch to another branch or commit
    Checkout,
    Stash,
    Pr,
}

/// Where an event stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventState {
    /// Over and done with
    #[default]
    Done,
    /// A commit the upstream doesn't have yet
    Pending,
    /// An open PR
    Open,
    /// A draft PR
    Draft,
    /// A merged PR
    Merged,
    /// A PR closed without merging
    Closed,
}

/// One row of the timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEvent {
    pub kind: EventKind,
    pub state: EventState,
    /// When, in Unix seconds
    pub at: i64,
    /// `Pushed 3 commits to origin/main`, a commit's subject
    pub title: String,
    /// `9984d116 origin/main`, `feature → main`
    pub detail: String,
    /// What opening it shows
    pub target: RepoTarget,
    /// Its web page (PRs)
    pub url: Option<String>,
}

/// A local branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoBranch {
    pub name: String,
    /// Whether it is checked out here
    pub current: bool,
    /// Its upstream (`origin/main`)
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    /// Whether its upstream was deleted
    pub gone: bool,
    /// When its newest commit was made, in Unix seconds
    pub at: i64,
    /// Its newest commit's subject
    pub subject: String,
}

/// A tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoTag {
    pub name: String,
    /// The tag's message, or its commit's subject
    pub subject: String,
    /// When it was made, in Unix seconds
    pub at: i64,
}

/// A worktree of the repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoWorktree {
    pub path: PathBuf,
    /// Branch checked out; `None` when detached
    pub branch: Option<String>,
    /// `HEAD`'s short sha
    pub head: String,
    /// Whether it is the one the agent works in
    pub current: bool,
    /// Whether `git worktree lock` keeps it
    pub locked: bool,
    /// Whether its folder is gone
    pub prunable: bool,
}

/// Whether the timeline lists the repo's PRs.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrState {
    /// The repo has no remote on github.com
    #[default]
    NotGitHub,
    /// Why they aren't listed (`Reading PRs…`, how to sign in)
    Note(String),
    /// They are, as [`EventKind::Pr`] events
    Listed,
}

/// What a REPO row opens: its diff, posted in the chat.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoTarget {
    /// The branch's commits the upstream lacks
    Unpushed,
    /// One uncommitted change; `from` is where a rename came from
    Change { path: String, from: Option<String> },
    /// One commit, against its first parent
    Commit { sha: String },
    /// What moved between two commits; with no `from`, since `to` left
    /// the upstream
    Range { from: Option<String>, to: String },
    /// A stash (`stash@{0}`)
    Stash { name: String },
    /// A PR on GitHub
    Pr { number: u64 },
    /// What a branch has that `HEAD` lacks
    Branch { name: String },
    /// What changed since a tag
    Tag { name: String },
    /// A worktree's uncommitted changes
    Worktree { path: PathBuf },
}

/// What the REPO tab asks of the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoRequest {
    /// Read the repo; `force` reads the PR list again too
    Load { force: bool },
    /// Post `target`'s diff in the chat under `title`
    Open { target: RepoTarget, title: String },
}
