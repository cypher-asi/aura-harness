//! `aura gate` subcommands.

use clap::{Args, Subcommand};
use std::path::PathBuf;

/// `aura gate` parent command.
#[derive(Args, Debug)]
pub struct GateCommand {
    /// Gate subcommand to run.
    #[command(subcommand)]
    pub action: GateSubcommand,
}

/// Subcommands under `aura gate`.
#[derive(Subcommand, Debug)]
pub enum GateSubcommand {
    /// Score Gate's verdicts, and each sign of a failed attempt, on the
    /// attempt log.
    Eval {
        /// Log to read (default: `gate/attempts.jsonl` under
        /// `AURA_DATA_DIR` or `./aura_data`).
        #[arg(long)]
        log: Option<PathBuf>,
        /// Only attempts whose model Gate chose.
        #[arg(long)]
        routed: bool,
        /// Only attempts Gate counted as passed.
        #[arg(long)]
        passed: bool,
        /// Only attempts at benchmark tasks whose id contains this.
        #[arg(long)]
        task: Option<String>,
        /// Print JSON instead of tables, with every scored attempt.
        #[arg(long)]
        json: bool,
    },
    /// Ask Gate's reviewer about logged attempts that a benchmark task's
    /// check proved right or wrong, from their kept copies and saved
    /// sessions, and log its answers for `aura gate eval` to score. This
    /// calls the model once per attempt.
    Replay {
        /// Data directory holding the log and the saved sessions
        /// (default: `AURA_DATA_DIR` or `./aura_data`).
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Only attempts at benchmark tasks whose id contains this.
        #[arg(long)]
        task: Option<String>,
        /// At most this many attempts, oldest first.
        #[arg(long)]
        limit: Option<usize>,
        /// Let the reviewer reason before it answers.
        #[arg(long)]
        thinking: bool,
        /// Attempts of any difficulty, not only those rated hard.
        #[arg(long)]
        all: bool,
        /// Model that reviews (default: the strongest model in the log).
        #[arg(long)]
        judge: Option<String>,
        /// List the attempts and what asking may cost, then stop.
        #[arg(long)]
        dry_run: bool,
        /// Ask again about attempts already replayed the same way.
        #[arg(long)]
        again: bool,
        /// Reviews asked at a time.
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        /// Hold the work to acceptance criteria written from its
        /// request, as a live turn's is, and have the reviewer go through
        /// them. Each request's criteria are written once and kept.
        #[arg(long)]
        criteria: bool,
        /// Model that writes the criteria (default: the latest
        /// standard-tier model in the log).
        #[arg(long, requires = "criteria")]
        writer: Option<String>,
    },
}
