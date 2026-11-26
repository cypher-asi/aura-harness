//! What pulling an agent's work back brought.

use std::path::PathBuf;

/// How pulling an agent's work back went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchOutcome {
    /// The machine changed nothing.
    Empty,
    /// The patch applied to the project folder.
    Applied {
        /// Files it touched.
        files: Vec<String>,
    },
    /// The patch did not apply; it is saved here.
    Saved {
        /// The saved patch.
        path: PathBuf,
        /// Why it did not apply.
        reason: String,
    },
}

impl PatchOutcome {
    /// A one-line report for the chat.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Empty => "no changes to pull".to_string(),
            Self::Applied { files } => format!("pulled {} changed file(s)", files.len()),
            Self::Saved { path, reason } => {
                format!(
                    "changes did not apply ({reason}); saved to {}",
                    path.display()
                )
            }
        }
    }
}

/// How bringing an agent's commits back went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitsOutcome {
    /// It made no commits the project lacks (or its machine predates
    /// agent branches).
    None,
    /// They are on the project's branch of the same name.
    Fetched {
        /// The branch.
        branch: String,
        /// How many came.
        commits: u64,
    },
    /// They stayed on the machine.
    Kept {
        /// The branch.
        branch: String,
        /// Why.
        reason: String,
    },
}

impl CommitsOutcome {
    /// A short report for the chat; `None` when there is nothing to say.
    #[must_use]
    pub fn describe(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Fetched { branch, commits } => {
                Some(format!("{commits} commit(s) on branch {branch}"))
            }
            Self::Kept { branch, reason } => {
                Some(format!("commits on {branch} not fetched: {reason}"))
            }
        }
    }
}

/// What pulling an agent's work back brought.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullOutcome {
    /// Its working tree's changes, applied to the project folder.
    pub patch: PatchOutcome,
    /// Its commits, as a branch here.
    pub commits: CommitsOutcome,
}

impl PullOutcome {
    /// Only a patch outcome.
    #[must_use]
    pub const fn patch_only(patch: PatchOutcome) -> Self {
        Self {
            patch,
            commits: CommitsOutcome::None,
        }
    }

    /// A one-line report for the chat.
    #[must_use]
    pub fn describe(&self) -> String {
        match self.commits.describe() {
            Some(commits) => format!("{}; {commits}", self.patch.describe()),
            None => self.patch.describe(),
        }
    }
}
