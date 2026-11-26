//! Boot consoles: a machine coming up, or an agent moving between machines.

#[cfg(doc)]
use crate::events::UiCommand;
use serde::{Deserialize, Serialize};

/// Where a step of a boot console is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BootStageState {
    /// Planned, not begun
    Pending,
    /// Under way
    Running,
    /// Finished after `ms`
    Done {
        /// Milliseconds it took
        ms: u64,
    },
    /// Not needed, and why
    Skipped(String),
    /// Failed, and why
    Failed(String),
}

/// A line of command output in a boot console.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootLogLine {
    /// Whether the command printed it on stderr
    pub stderr: bool,
    /// The line, without terminal escapes
    pub text: String,
}

/// A step a boot console expects to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootPlanStep {
    /// Stable key of the step
    pub key: String,
    /// What the step does, e.g. `Creating machine`
    pub label: String,
}

/// A change to a boot console ([`UiCommand::Boot`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BootUpdate {
    /// The console starts, headed `title`, e.g. `Moving to the cloud`
    Begin {
        /// What is happening
        title: String,
    },
    /// The steps to come, in order; a step that starts unplanned goes in
    /// before those not begun
    Plan(Vec<BootPlanStep>),
    /// A step started, finished, failed or was skipped
    Stage {
        /// Stable key of the step
        key: String,
        /// What the step does, e.g. `Creating machine`
        label: String,
        /// Where it is
        state: BootStageState,
    },
    /// Something known about the machine; replaces an earlier fact with
    /// the same key
    Fact {
        /// What it is, e.g. `region`
        key: String,
        /// Its value
        value: String,
    },
    /// Lines commands printed, oldest first
    Log(Vec<BootLogLine>),
    /// How far a step has got
    Progress {
        /// Stable key of the step
        key: String,
        /// Done so far
        done: u64,
        /// Out of
        total: u64,
    },
    /// What a running step waits on now and how long it may take;
    /// `what: None` clears it. One replaced without being cleared first
    /// keeps its clock. Never kept in history: it is about the moment.
    Waiting {
        /// Stable key of the step
        key: String,
        /// The call or command pending
        what: Option<String>,
        /// When it gives up, ms, if it does
        limit_ms: Option<u64>,
    },
    /// The console is done; `summary` says how it went
    End {
        /// Whether it went through
        ok: bool,
        /// One line on how it went
        summary: String,
    },
}

/// A boot console as its updates, to show it again in a chat rebuilt
/// from history. Its `timestamp_ms` is when it began.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootHistory {
    /// Console id; later [`UiCommand::Boot`]s for it change it
    pub id: String,
    /// Its updates, oldest first
    pub updates: Vec<BootUpdate>,
    /// How long it took, once it ended
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
}
