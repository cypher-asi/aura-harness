//! `aura eval` and `aura dataset` subcommands.

use super::HarnessArgs;
use clap::{Args, Subcommand};
use std::path::PathBuf;

/// `aura eval` parent command.
#[derive(Args, Debug)]
pub struct EvalCommand {
    /// Eval subcommand to run.
    #[command(subcommand)]
    pub action: EvalSubcommand,
}

/// Subcommands under `aura eval`.
#[derive(Subcommand, Debug)]
pub enum EvalSubcommand {
    /// Run a task suite and record each task as an eval run.
    Run {
        /// Suite directory (one `<task>/task.toml` per task).
        #[arg(default_value = "evals/coding")]
        suite: PathBuf,
        /// Only run these tasks; repeatable.
        #[arg(long)]
        task: Vec<String>,
        /// Runs per task.
        #[arg(long, default_value_t = 1)]
        repeat: usize,
        /// Where task workspaces are created (default `AURA_HOME/evals/work`).
        #[arg(long)]
        work_dir: Option<PathBuf>,
        /// Stop starting runs once the suite's runs cost this many USD.
        #[arg(long)]
        budget_usd: Option<f64>,
        #[command(flatten)]
        harness: HarnessArgs,
    },
    /// Compare recorded eval runs across arms.
    Compare {
        /// Only runs of this suite.
        #[arg(long)]
        suite: Option<String>,
        /// Arms to compare, baseline first; repeatable (default: all).
        #[arg(long = "arm")]
        arms: Vec<String>,
        /// Relative change tolerated before a metric counts as a regression.
        #[arg(long, default_value_t = 0.1)]
        tolerance: f64,
        /// Exit with an error when any arm regresses against the baseline.
        #[arg(long)]
        fail_on_regression: bool,
        /// Compare against this baseline file instead of the first arm.
        #[arg(long, conflicts_with = "write_baseline")]
        baseline: Option<PathBuf>,
        /// Write the first arm's numbers to this baseline file.
        #[arg(long)]
        write_baseline: Option<PathBuf>,
    },
}

/// `aura dataset` parent command.
#[derive(Args, Debug)]
pub struct DatasetCommand {
    /// Dataset subcommand to run.
    #[command(subcommand)]
    pub action: DatasetSubcommand,
}

/// Subcommands under `aura dataset`.
#[derive(Subcommand, Debug)]
pub enum DatasetSubcommand {
    /// Export runs as `train.jsonl` / `eval.jsonl` plus a dataset card.
    Build {
        /// `sft`, `preference`, `rl` or `harness-bandit`.
        #[arg(long)]
        format: String,
        /// Comma-separated filter, e.g. `outcome>=0.5,model=claude-opus-4-6`.
        #[arg(long, default_value = "")]
        filter: String,
        /// Output directory (default `AURA_HOME/datasets/<format>`).
        #[arg(long)]
        out: Option<PathBuf>,
    },
}
