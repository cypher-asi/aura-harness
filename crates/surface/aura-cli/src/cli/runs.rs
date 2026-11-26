//! `aura runs` subcommands and the harness settings shared with `aura eval`.

use clap::{Args, Subcommand};
use std::path::PathBuf;

/// `aura runs` parent command.
#[derive(Args, Debug)]
pub struct RunsCommand {
    /// Runs subcommand to run.
    #[command(subcommand)]
    pub action: RunsSubcommand,
}

/// Subcommands under `aura runs`.
#[derive(Subcommand, Debug)]
pub enum RunsSubcommand {
    /// List recorded runs, newest first.
    List {
        /// Show at most this many runs.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Show a run's scorecard and steps.
    Show {
        /// Run id, or a unique prefix or suffix of one.
        id: String,
        /// Also list every step.
        #[arg(long)]
        steps: bool,
        /// Print the normalized trace as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Check whether a run got stuck, and where.
    ///
    /// Works on runs that never ended too. Reports unfinished or
    /// cancelled tool calls, long approval waits and calls with no result
    /// step, and exits non-zero when it finds a problem.
    Check {
        /// Run id, or a unique prefix or suffix of one.
        id: String,
        /// Seconds after which a waiting tool call counts as a problem.
        #[arg(long, default_value_t = 30)]
        slow_secs: u64,
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Compare two runs' scorecards and flag regressions.
    Diff {
        /// Baseline run id.
        baseline: String,
        /// Candidate run id.
        candidate: String,
        /// Relative change tolerated before a metric counts as a regression.
        #[arg(long, default_value_t = 0.1)]
        tolerance: f64,
    },
    /// Attach a human label to a run or one of its steps.
    Label {
        /// Run id.
        id: String,
        /// Score; positive is good (for example 1 or -1).
        #[arg(allow_hyphen_values = true)]
        value: f64,
        /// Step sequence number to label instead of the whole run.
        #[arg(long)]
        step: Option<u64>,
        /// Label name.
        #[arg(long, default_value = "human")]
        name: String,
        /// Free-text note stored with the label.
        #[arg(long)]
        note: Option<String>,
    },
    /// Run a check in the run's workspace and record it as the outcome.
    Verify {
        /// Run id.
        id: String,
        /// Shell command whose exit status decides the outcome.
        #[arg(long)]
        cmd: String,
    },
    /// Re-run a recorded run from one of its steps.
    ///
    /// Model responses recorded before the step are replayed; the step
    /// and everything after it run live against `--workspace`.
    Branch {
        /// Run id.
        id: String,
        /// Step whose decision is made again.
        #[arg(long)]
        from_step: u64,
        /// Workspace to run in, in the state the original run started from
        /// (for example a fresh checkout); tools run there for real.
        #[arg(long)]
        workspace: PathBuf,
        #[command(flatten)]
        harness: HarnessArgs,
    },
}

/// Model and harness settings for headless runs.
#[derive(Args, Debug, Clone, Default)]
pub struct HarnessArgs {
    /// Model id, or `gate` for Gate's pick (default: the recorded run's
    /// model, or the terminal default).
    #[arg(long)]
    pub model: Option<String>,
    /// `routed` (default) or `mock`.
    #[arg(long, default_value = "routed")]
    pub provider: String,
    /// Harness parameter override `key=value`; repeatable.
    #[arg(long = "set", value_name = "KEY=VALUE")]
    pub set: Vec<String>,
    /// Arm label recorded with the run (default: the `--set` overrides).
    #[arg(long)]
    pub arm: Option<String>,
}
