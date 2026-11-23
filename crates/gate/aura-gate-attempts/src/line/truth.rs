//! A `truth` line: something outside an attempt that says whether its
//! work was right.

use std::path::PathBuf;

use aura_core_types::OutcomeSignal;
use serde::{Deserialize, Serialize};

/// What said whether an attempt's work was right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruthSource {
    /// A benchmark task's own check, run on the work after the run
    TaskCheck,
    /// The user rewound the turn
    Rewind,
    /// The user sent the prompt again to a stronger model
    RetryStronger,
    /// The user picked between two runs of `/compare`, in older logs
    Picked,
}

impl TruthSource {
    /// The source an `Outcome` with `signal` is, when the signal is the
    /// user's.
    #[must_use]
    pub const fn of_user(signal: OutcomeSignal) -> Option<Self> {
        match signal {
            OutcomeSignal::Rewind => Some(Self::Rewind),
            OutcomeSignal::RetryStronger => Some(Self::RetryStronger),
            _ => None,
        }
    }

    /// The task's own check outranks what the user did, which says less
    /// about the work itself.
    pub(crate) const fn rank(self) -> u8 {
        match self {
            Self::TaskCheck => 2,
            Self::Rewind | Self::RetryStronger | Self::Picked => 1,
        }
    }
}

/// Something outside an attempt that says whether its work was right.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Truth {
    /// When the line was written, in Unix milliseconds
    pub at_ms: u64,
    /// The attempt's agent, in hex
    pub agent: String,
    /// The attempt's prompt, in hex
    pub prompt_tx: String,
    /// What said so
    pub source: TruthSource,
    /// Whether the work was right
    pub passed: bool,
    /// The comparison the attempt's run belongs to, e.g. `Compare #22`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare: Option<String>,
    /// The run, e.g. `a-4`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The benchmark task the run did
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Where the run did its work, when it worked in a copy
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<CopyAt>,
}

/// The copy of the workspace a run of `/compare` worked in, kept so what
/// the run changed can be read again while the copy is there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyAt {
    /// Root of the copy
    pub root: PathBuf,
    /// Git directory kept beside a plain copy; absent for a worktree
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_dir: Option<PathBuf>,
    /// Git tree of the copy before the run
    pub baseline: String,
}
