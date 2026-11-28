//! What an entry's run tells its agent and the Queue tab beyond a line
//! of text: the PR it landed through, why it failed (the conflicted
//! files, the failing tests and their output), and why the queue holds.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A pull request on the forge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrRef {
    /// Its number in the repo (`#12`)
    pub number: u64,
    /// Its web page
    pub url: String,
}

/// What kind of problem stopped an entry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// The branch conflicts with the target in files the queue can't
    /// settle on its own
    Conflict,
    /// The build or a test failed on the merge
    Check,
    /// The forge refused the merge (closed PR, protection, permissions)
    Refused,
    /// Anything else: a missing branch, a git error, a timeout
    #[default]
    Other,
}

/// Why an entry didn't merge.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    /// What kind of problem it was
    #[serde(default)]
    pub kind: FailureKind,
    /// One readable reason, as the entry's detail says it
    #[serde(default)]
    pub reason: String,
    /// Files that conflicted
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Tests that failed
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<String>,
    /// The part of the output that shows the problem (a test's assertion
    /// or snapshot diff, else the end of the log)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    /// The PR the entry was landing through
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<PrRef>,
}

impl Failure {
    /// A failure of `kind` with `reason` and nothing else known.
    #[must_use]
    pub fn new(kind: FailureKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            reason: reason.into(),
            ..Self::default()
        }
    }
}

impl From<String> for Failure {
    fn from(reason: String) -> Self {
        Self::new(FailureKind::Other, reason)
    }
}

impl From<&str> for Failure {
    fn from(reason: &str) -> Self {
        Self::new(FailureKind::Other, reason)
    }
}

/// Why the queue stopped taking entries for a while. The entry that ran
/// into it goes back to its place, uncharged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    /// What is wrong, for the Queue tab and the reports
    pub reason: String,
    /// What kind of hold it is
    pub kind: HoldKind,
    /// When it started
    pub since: DateTime<Utc>,
    /// When the queue tries again regardless
    pub retry_at: DateTime<Utc>,
    /// The target's tip it was held at; a new tip ends the hold early
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_tip: Option<String>,
    /// Tests that fail on the plain target too
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<String>,
}

/// What a [`Hold`] waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldKind {
    /// The forge (GitHub) couldn't be reached or refused the queue's
    /// credentials; nothing is wrong with the entry
    ForgeUnavailable,
    /// The target itself fails the checks
    TargetBroken,
}
