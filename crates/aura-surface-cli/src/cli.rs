//! CLI argument definitions and parsing.

use crate::ModeFlag;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "aura",
    about = "AURA CLI - Autonomous Universal Reasoning Architecture"
)]
pub struct Cli {
    /// Session mode for the interactive terminal. Overrides the daemon
    /// default (`AURA_DEFAULT_MODE`); the kernel denies any tool call the
    /// mode does not allow.
    #[arg(long = "mode", value_enum, global = true)]
    pub mode: Option<ModeFlag>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run the agent (default when no subcommand is given).
    Run(RunArgs),
    /// Sign in to zOS, or link a Claude / ChatGPT plan.
    Login(LoginArgs),
    /// Sign out of zOS, or unlink a plan.
    Logout(LogoutArgs),
    /// Show the zOS sign-in and linked plans.
    Whoami,
    /// Manage linked plans.
    Auth(AuthCommand),
    /// Print "Hello, World!" and exit (Spec 01).
    Hello,
    /// Phase 4a stub: migrate aura state (no-op today).
    ///
    /// Future phases will populate this with Codex → Aura state
    /// migration. The stub exists so the CLI surface is stable when
    /// `aura-store-db` and `aura-plugin-core` (Phase 4b+) start
    /// producing migration-aware on-disk layouts.
    Migrate(MigrateArgs),
    /// Manage declarative plugins (Phase 4b).
    ///
    /// Subcommands install / list / enable / disable interact with
    /// the on-disk plugin cache under `AURA_HOME/plugins/` and the
    /// `[plugins.<id>]` table inside `AURA_HOME/config.toml`. No
    /// agent-loop wiring lands until Phase 4c (hooks/MCP/connectors)
    /// and Phase 8 (full integration).
    Plugins(PluginsCommand),
    /// Inspect, compare and label recorded runs (`AURA_HOME/runs/`).
    Runs(RunsCommand),
    /// Export recorded runs as training data.
    Dataset(DatasetCommand),
    /// Run coding eval suites and compare harness arms.
    Eval(EvalCommand),
}

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
    /// Model id (default: the recorded run's model, or the terminal default).
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

/// Arguments for `aura login`.
#[derive(Args, Debug)]
pub struct LoginArgs {
    /// `zos` (default) for cloud models via aura-router, or `claude` /
    /// `chatgpt` to use your own plan directly.
    #[arg(default_value = "zos")]
    pub provider: String,
    /// Paste the code shown by the provider instead of using a local
    /// browser redirect (for SSH and headless machines).
    #[arg(long)]
    pub paste: bool,
}

/// Arguments for `aura logout`.
#[derive(Args, Debug)]
pub struct LogoutArgs {
    /// `zos` (default) to sign out, or `claude` / `chatgpt` to unlink.
    #[arg(default_value = "zos", conflicts_with = "all")]
    pub provider: String,
    /// Unlink every plan and sign out of zOS.
    #[arg(long)]
    pub all: bool,
}

/// `aura auth` parent command.
#[derive(Args, Debug)]
pub struct AuthCommand {
    #[command(subcommand)]
    pub action: AuthSubcommand,
}

/// Subcommands under `aura auth`.
#[derive(Subcommand, Debug)]
pub enum AuthSubcommand {
    /// Attach plans linked while signed out to the signed-in zOS account.
    Move {
        /// Move to the signed-in zOS account (required).
        #[arg(long, required = true)]
        to_zos: bool,
    },
}

/// Arguments for the `migrate` subcommand (Phase 4a stub).
#[derive(Args, Debug)]
pub struct MigrateArgs {
    /// Run as a dry preview without making changes.
    #[arg(long)]
    pub dry_run: bool,
}

/// `aura plugins` parent command (Phase 4b).
#[derive(Args, Debug)]
pub struct PluginsCommand {
    /// Plugins subcommand to run.
    #[command(subcommand)]
    pub action: PluginsSubcommand,
}

/// Subcommands under `aura plugins` (Phase 4b).
#[derive(Subcommand, Debug)]
pub enum PluginsSubcommand {
    /// Install a plugin from a local source directory.
    ///
    /// The source must contain a `.aura-plugin/`, `.codex-plugin/`,
    /// or `.claude-plugin/` subdirectory with a `manifest.toml`. The
    /// install pipeline copies the source tree into the
    /// `AURA_HOME/plugins/<id>/<version>/` cache layout and writes a
    /// normalised `.aura-plugin.toml` regardless of which alias the
    /// source used.
    Install {
        /// Path to the plugin source directory.
        source: PathBuf,
        /// Bypass `trust.require_explicit_trust = true` manifests.
        #[arg(long)]
        trust: bool,
    },
    /// List installed plugins and their active versions.
    List,
    /// Enable a plugin in `AURA_HOME/config.toml` (`enabled = true`).
    ///
    /// When the cached manifest has `trust.require_explicit_trust = true`
    /// and the plugin is not already trusted in operator config, the
    /// flow prompts the operator on the TTY. `--yes` skips the
    /// prompt and accepts trust; `--no` skips the prompt and
    /// declines (the plugin is NOT enabled).
    Enable {
        /// Plugin id (matches the manifest `id` field).
        id: String,
        /// Bypass the trust prompt and accept trust automatically.
        /// Mutually exclusive with `--no`.
        #[arg(long, conflicts_with = "no")]
        yes: bool,
        /// Bypass the trust prompt and decline trust automatically.
        /// Mutually exclusive with `--yes`.
        #[arg(long, conflicts_with = "yes")]
        no: bool,
    },
    /// Disable a plugin in `AURA_HOME/config.toml` (`enabled = false`).
    Disable {
        /// Plugin id (matches the manifest `id` field).
        id: String,
    },
}

/// Arguments for the `run` subcommand (also the default behaviour).
#[derive(Parser)]
pub struct RunArgs {
    /// UI mode (terminal or none)
    #[arg(long, default_value = "terminal")]
    pub ui: UiMode,

    /// Theme (cyber, matrix, synthwave, minimal)
    #[arg(long, default_value = "cyber")]
    pub theme: String,

    /// Working directory
    #[arg(short, long)]
    pub dir: Option<PathBuf>,

    /// Model provider: `anthropic` (local Ollama models + aura-router) or `mock`
    #[arg(long, default_value = "anthropic")]
    pub provider: String,

    /// Enable verbose output
    #[arg(short, long)]
    pub verbose: bool,

    /// Permit FullAccess sessions to bypass command allowlists.
    #[arg(long)]
    pub allow_unrestricted_full_access: bool,
}

impl Default for RunArgs {
    fn default() -> Self {
        Self {
            ui: UiMode::Terminal,
            theme: "cyber".to_string(),
            dir: None,
            provider: "anthropic".to_string(),
            verbose: false,
            allow_unrestricted_full_access: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum UiMode {
    /// Full terminal UI (default)
    Terminal,
    /// No UI, run as swarm server
    None,
}
