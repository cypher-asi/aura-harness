//! The filters of the REPO tab's strip: what each shows, and which a
//! view has anything for.

use crate::repo::{EventKind, PrState, RepoView};

/// What the REPO tab's list shows, picked in the strip at its top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum RepoFilter {
    /// The sync state, the changes and every event, newest first
    #[default]
    All,
    /// The uncommitted changes alone
    Changes,
    Commits,
    Pushes,
    /// Pulls, and fetches that moved a remote-tracking branch
    Pulls,
    Prs,
    /// Merges and rebases
    Merges,
    Resets,
    /// Switches to another branch or commit
    Checkouts,
    Stashes,
    /// The repo's local branches
    Branches,
    Tags,
    Worktrees,
}

impl RepoFilter {
    /// Every filter, in the strip's order.
    pub const ALL: [Self; 13] = [
        Self::All,
        Self::Changes,
        Self::Commits,
        Self::Pushes,
        Self::Pulls,
        Self::Prs,
        Self::Merges,
        Self::Resets,
        Self::Checkouts,
        Self::Stashes,
        Self::Branches,
        Self::Tags,
        Self::Worktrees,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Changes => "Changes",
            Self::Commits => "Commits",
            Self::Pushes => "Pushes",
            Self::Pulls => "Pulls",
            Self::Prs => "PRs",
            Self::Merges => "Merges",
            Self::Resets => "Resets",
            Self::Checkouts => "Checkouts",
            Self::Stashes => "Stashes",
            Self::Branches => "Branches",
            Self::Tags => "Tags",
            Self::Worktrees => "Worktrees",
        }
    }

    /// Whether it lists the repo's branches, tags or worktrees rather than
    /// a timeline; the strip sets these apart behind a bar.
    #[must_use]
    pub const fn is_list(self) -> bool {
        matches!(self, Self::Branches | Self::Tags | Self::Worktrees)
    }

    /// Whether its timeline shows events of `kind`.
    #[must_use]
    pub const fn shows(self, kind: EventKind) -> bool {
        match self {
            Self::All => true,
            Self::Commits => matches!(kind, EventKind::Commit),
            Self::Pushes => matches!(kind, EventKind::Push),
            Self::Pulls => matches!(kind, EventKind::Pull | EventKind::Fetch),
            Self::Prs => matches!(kind, EventKind::Pr),
            Self::Merges => matches!(kind, EventKind::Merge | EventKind::Rebase),
            Self::Resets => matches!(kind, EventKind::Reset),
            Self::Checkouts => matches!(kind, EventKind::Checkout),
            Self::Stashes => matches!(kind, EventKind::Stash),
            Self::Changes | Self::Branches | Self::Tags | Self::Worktrees => false,
        }
    }

    /// Whether `view` has anything to show under it, which puts it in the
    /// strip. PRs are there whenever the repo is on GitHub, to say why
    /// none are listed when they can't be.
    #[must_use]
    pub fn has_any(self, view: &RepoView) -> bool {
        match self {
            Self::All => true,
            Self::Changes => !view.changes.is_empty(),
            Self::Prs => view.prs != PrState::NotGitHub,
            Self::Branches => !view.branches.is_empty(),
            Self::Tags => !view.tags.is_empty(),
            Self::Worktrees => view.worktrees.len() > 1,
            timeline => view.events.iter().any(|event| timeline.shows(event.kind)),
        }
    }

    /// The filter `/repo <word>` names: its label, singular or plural, in
    /// any case.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        let word = word.trim().to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|filter| {
                let label = filter.label().to_ascii_lowercase();
                let one = |suffix: &str| label.strip_suffix(suffix).is_some_and(|one| one == word);
                label == word || one("es") || one("s")
            })
            .or(match word.as_str() {
                "fetch" | "fetches" => Some(Self::Pulls),
                "rebase" | "rebases" => Some(Self::Merges),
                "switch" | "switches" => Some(Self::Checkouts),
                _ => None,
            })
    }
}
