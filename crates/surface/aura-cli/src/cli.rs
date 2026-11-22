//! CLI argument definitions and parsing.

use crate::{ApprovalFlag, ModeFlag};
use aura_app::headless::HeadlessOptions;
use aura_app::session::SessionOptions;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "aura",
    about = "AURA CLI - Autonomous Universal Reasoning Architecture"
)]
pub struct Cli {
    /// Session mode for the desktop window or terminal. Overrides the daemon
    /// default (`AURA_DEFAULT_MODE`); the kernel denies any tool call the
    /// mode does not allow.
    #[arg(long = "mode", value_enum, global = true)]
    pub mode: Option<ModeFlag>,

    /// What tool calls ask first: `yolo` runs everything with commands
    /// confined to writing inside the working folder, `ask` asks for
    /// anything not on the allow list, `strict` asks for everything but
    /// reads. Overrides `AURA_TUI_APPROVAL`; `-p` defaults to `yolo` in
    /// agent mode.
    #[arg(long = "approval", value_enum, global = true)]
    pub approval: Option<ApprovalFlag>,

    /// Run one prompt without the terminal UI, print the result, and exit.
    /// `-p -`, or `-p` with no prompt, reads the prompt from stdin; piped
    /// stdin is appended to a prompt given here.
    #[arg(
        short = 'p',
        long = "print",
        value_name = "PROMPT",
        num_args = 0..=1,
        default_missing_value = "-"
    )]
    pub print: Option<String>,

    /// Run the agent in this terminal instead of the desktop window.
    /// Without a display (e.g. over SSH) the terminal is used anyway.
    #[arg(long, visible_alias = "tui", conflicts_with = "print")]
    pub terminal: bool,

    #[command(flatten)]
    pub headless: HeadlessArgs,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Options for `aura -p`. Each one requires `-p`.
#[derive(Args, Debug, Clone, Default)]
#[command(next_help_heading = "Options for -p")]
pub struct HeadlessArgs {
    /// What `-p` prints: the final reply (`text`), one result object
    /// (`json`), or one event per line ending with the result
    /// (`stream-json`).
    #[arg(long, value_enum, value_name = "FORMAT", requires = "print")]
    pub output_format: Option<OutputFormat>,

    /// Continue the saved session with this id (the `session_id` of an
    /// earlier run).
    #[arg(
        long,
        value_name = "SESSION_ID",
        requires = "print",
        conflicts_with = "continue_session"
    )]
    pub resume: Option<String>,

    /// Continue the most recent `aura -p` session.
    #[arg(short = 'c', long = "continue", requires = "print")]
    pub continue_session: bool,

    /// Only these tools may run (comma-separated); every other tool is
    /// denied.
    #[arg(
        long,
        value_name = "TOOLS",
        value_delimiter = ',',
        num_args = 1..,
        requires = "print"
    )]
    pub allowed_tools: Vec<String>,

    /// These tools may not run (comma-separated).
    #[arg(
        long,
        value_name = "TOOLS",
        value_delimiter = ',',
        num_args = 1..,
        requires = "print"
    )]
    pub disallowed_tools: Vec<String>,

    /// Stop after this many model calls.
    #[arg(
        long,
        value_name = "N",
        requires = "print",
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    pub max_turns: Option<u32>,

    /// Stop after this long: `90s`, `45m`, `2h`, or a bare number of
    /// minutes. Overrides `AURA_TUI_TURN_TIMEOUT`.
    #[arg(long, value_name = "DURATION", requires = "print", value_parser = parse_duration_arg)]
    pub max_time: Option<Duration>,

    /// Stop once model calls cost more than this many US dollars at API
    /// list prices. Overrides `AURA_TUI_MAX_TURN_COST_USD`.
    #[arg(long, value_name = "USD", requires = "print", value_parser = parse_cost_arg)]
    pub max_cost_usd: Option<f64>,

    /// Work in this directory instead of the current one.
    #[arg(short = 'd', long = "cwd", value_name = "DIR", requires = "print")]
    pub cwd: Option<PathBuf>,

    /// Model for this run, or `gate` to let Gate pick one from what you
    /// are signed in to. Not saved as the `/model` choice.
    #[arg(long, value_name = "MODEL", requires = "print")]
    pub model: Option<String>,

    /// Model provider: `anthropic` (default; local, plan and aura-router
    /// models) or `mock`.
    #[arg(long, value_name = "PROVIDER", requires = "print")]
    pub provider: Option<String>,

    /// Log debug detail to stderr.
    #[arg(long, requires = "print")]
    pub verbose: bool,
}

/// How `aura -p` prints its result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// The final reply as plain text.
    #[default]
    Text,
    /// One JSON object describing the run.
    Json,
    /// Newline-delimited JSON events, ending with the result object.
    StreamJson,
}

impl From<OutputFormat> for aura_app::headless::OutputFormat {
    fn from(value: OutputFormat) -> Self {
        match value {
            OutputFormat::Text => Self::Text,
            OutputFormat::Json => Self::Json,
            OutputFormat::StreamJson => Self::StreamJson,
        }
    }
}

impl From<HeadlessArgs> for HeadlessOptions {
    fn from(args: HeadlessArgs) -> Self {
        Self {
            output_format: args.output_format.unwrap_or_default().into(),
            resume: args.resume,
            continue_session: args.continue_session,
            allowed_tools: args.allowed_tools,
            disallowed_tools: args.disallowed_tools,
            max_turns: args.max_turns,
            max_time: args.max_time,
            max_cost_usd: args.max_cost_usd,
            cwd: args.cwd,
            model: args.model,
            provider: args.provider,
            verbose: args.verbose,
            approval: None,
        }
    }
}

fn parse_duration_arg(raw: &str) -> Result<Duration, String> {
    aura_app::parse_duration(raw)
        .ok_or_else(|| format!("`{raw}` is not a duration like 90s, 45m or 2h"))
}

fn parse_cost_arg(raw: &str) -> Result<f64, String> {
    raw.trim()
        .parse::<f64>()
        .ok()
        .filter(|usd| usd.is_finite() && *usd > 0.0)
        .ok_or_else(|| format!("`{raw}` is not a positive number of US dollars"))
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run the agent in the desktop window (default with no subcommand).
    Desktop(DesktopArgs),
    /// Run the agent in the terminal, or as a server with `--ui none`.
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
    /// `aura-store-db` and `aura-plugin` (Phase 4b+) start
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
    /// Check Gate's verdicts against what happened.
    Gate(GateCommand),
}

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

impl RunArgs {
    /// The session settings these arguments ask for.
    #[must_use]
    pub fn session_options(&self) -> SessionOptions {
        SessionOptions {
            dir: self.dir.clone(),
            provider: self.provider.clone(),
            allow_unrestricted_full_access: self.allow_unrestricted_full_access,
            approval: None,
        }
    }
}

/// Options for the desktop window (`aura desktop`, or `aura` alone).
#[derive(Args, Debug, Clone)]
pub struct DesktopArgs {
    /// Folder the agent works in (default: the current directory)
    #[arg(short, long)]
    pub workspace: Option<PathBuf>,

    /// Data directory for agents and records (default: `AURA_DATA_DIR` or `./aura_data`)
    #[arg(short, long)]
    pub dir: Option<PathBuf>,

    /// Model provider: `anthropic` (local Ollama models + aura-router) or `mock`
    #[arg(long, default_value = "anthropic")]
    pub provider: String,

    /// Theme (cyber, matrix, synthwave, minimal)
    #[arg(long, default_value = "cyber")]
    pub theme: String,

    /// Log debug output to stderr
    #[arg(short, long)]
    pub verbose: bool,

    /// Permit FullAccess sessions to bypass command allowlists
    #[arg(long)]
    pub allow_unrestricted_full_access: bool,

    /// Run against a scripted session instead of a real agent
    #[arg(long)]
    pub demo: bool,
}

impl Default for DesktopArgs {
    fn default() -> Self {
        Self {
            workspace: None,
            dir: None,
            provider: "anthropic".to_string(),
            theme: "cyber".to_string(),
            verbose: false,
            allow_unrestricted_full_access: false,
            demo: false,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("aura").chain(args.iter().copied()))
    }

    #[test]
    fn print_takes_a_prompt_or_reads_stdin() {
        let cli = parse(&["-p", "fix the tests"]).unwrap();
        assert_eq!(cli.print.as_deref(), Some("fix the tests"));
        assert!(cli.command.is_none());

        assert_eq!(parse(&["-p"]).unwrap().print.as_deref(), Some("-"));
        assert_eq!(
            parse(&["--print", "-"]).unwrap().print.as_deref(),
            Some("-")
        );
        assert_eq!(
            parse(&["-p", "--output-format", "json"])
                .unwrap()
                .print
                .as_deref(),
            Some("-")
        );
        assert!(parse(&[]).unwrap().print.is_none());
    }

    #[test]
    fn gate_eval_parses() {
        let eval = |args: &[&str]| match parse(args).unwrap().command {
            Some(Commands::Gate(GateCommand {
                action:
                    GateSubcommand::Eval {
                        log,
                        routed,
                        passed,
                        task,
                        json,
                    },
            })) => (log, routed, passed, task, json),
            _ => panic!("not gate eval: {args:?}"),
        };
        assert_eq!(eval(&["gate", "eval"]), (None, false, false, None, false));
        assert_eq!(
            eval(&[
                "gate",
                "eval",
                "--log",
                "/tmp/attempts.jsonl",
                "--routed",
                "--passed",
                "--task",
                "swe_bench",
                "--json"
            ]),
            (
                Some(PathBuf::from("/tmp/attempts.jsonl")),
                true,
                true,
                Some("swe_bench".to_string()),
                true
            )
        );
    }

    #[test]
    fn gate_replay_parses() {
        let replay = |args: &[&str]| match parse(args).unwrap().command {
            Some(Commands::Gate(GateCommand {
                action:
                    GateSubcommand::Replay {
                        data_dir,
                        task,
                        limit,
                        thinking,
                        all,
                        judge,
                        dry_run,
                        again,
                        jobs,
                        criteria,
                        writer,
                    },
            })) => (
                data_dir,
                task,
                limit,
                (thinking, all, dry_run, again, criteria),
                (judge, writer),
                jobs,
            ),
            _ => panic!("not gate replay: {args:?}"),
        };
        assert_eq!(
            replay(&["gate", "replay"]),
            (
                None,
                None,
                None,
                (false, false, false, false, false),
                (None, None),
                4
            )
        );
        assert!(
            parse(&["gate", "replay", "--writer", "claude/claude-sonnet-5-5"]).is_err(),
            "a writer is named only with --criteria"
        );
        assert_eq!(
            replay(&[
                "gate",
                "replay",
                "--data-dir",
                "/tmp/data",
                "--task",
                "ansible",
                "--limit",
                "5",
                "--thinking",
                "--all",
                "--judge",
                "claude/claude-opus-5-5",
                "--dry-run",
                "--again",
                "--jobs",
                "2",
                "--criteria",
                "--writer",
                "claude/claude-sonnet-5-5"
            ]),
            (
                Some(PathBuf::from("/tmp/data")),
                Some("ansible".to_string()),
                Some(5),
                (true, true, true, true, true),
                (
                    Some("claude/claude-opus-5-5".to_string()),
                    Some("claude/claude-sonnet-5-5".to_string())
                ),
                2
            )
        );
    }

    #[test]
    fn a_prompt_naming_a_subcommand_is_still_the_prompt() {
        let cli = parse(&["-p", "hello"]).unwrap();
        assert_eq!(cli.print.as_deref(), Some("hello"));
        assert!(cli.command.is_none());
    }

    #[test]
    fn headless_flags_parse() {
        let cli = parse(&[
            "-p",
            "go",
            "--output-format",
            "stream-json",
            "--resume",
            "abc:1",
            "--allowed-tools",
            "read_file,run_command",
            "--disallowed-tools",
            "write_file",
            "--max-turns",
            "12",
            "--max-time",
            "90s",
            "--max-cost-usd",
            "2.5",
            "-d",
            "/tmp/project",
            "--model",
            "claude/claude-sonnet-4-5",
            "--provider",
            "mock",
            "--mode",
            "plan",
        ])
        .unwrap();
        let h = &cli.headless;
        assert_eq!(h.output_format, Some(OutputFormat::StreamJson));
        assert_eq!(h.resume.as_deref(), Some("abc:1"));
        assert_eq!(h.allowed_tools, ["read_file", "run_command"]);
        assert_eq!(h.disallowed_tools, ["write_file"]);
        assert_eq!(h.max_turns, Some(12));
        assert_eq!(h.max_time, Some(Duration::from_secs(90)));
        assert_eq!(h.max_cost_usd, Some(2.5));
        assert_eq!(h.cwd.as_deref(), Some(std::path::Path::new("/tmp/project")));
        assert_eq!(h.model.as_deref(), Some("claude/claude-sonnet-4-5"));
        assert_eq!(h.provider.as_deref(), Some("mock"));
        assert_eq!(cli.mode, Some(ModeFlag::Plan));
    }

    #[test]
    fn only_bare_aura_and_aura_desktop_open_the_window() {
        let desktop = |args: &[&str]| crate::desktop_launch(&parse(args).unwrap());
        let window = desktop(&["desktop", "--demo"]).expect("aura desktop opens the window");
        assert!(window.demo);
        assert_eq!(window.provider, "anthropic");
        for args in [
            &["--terminal"][..],
            &["--tui", "--mode", "plan"],
            &["run"],
            &["-p", "x"],
            &["runs", "list"],
        ] {
            assert!(desktop(args).is_none(), "{args:?} opened the window");
        }
        assert!(parse(&["--terminal", "-p", "x"]).is_err());
    }

    #[test]
    fn continue_and_resume_exclude_each_other() {
        assert!(parse(&["-p", "x", "-c"]).unwrap().headless.continue_session);
        assert!(parse(&["-p", "x", "--continue", "--resume", "a:1"]).is_err());
    }

    #[test]
    fn headless_flags_require_print() {
        for args in [
            &["--output-format", "json"][..],
            &["--continue"],
            &["--max-turns", "3"],
            &["--cwd", "/tmp"],
            &["--allowed-tools", "read_file"],
        ] {
            assert!(parse(args).is_err(), "{args:?} parsed without -p");
        }
    }

    #[test]
    fn limits_reject_nonsense() {
        assert!(parse(&["-p", "x", "--max-turns", "0"]).is_err());
        assert!(parse(&["-p", "x", "--max-time", "soon"]).is_err());
        assert!(parse(&["-p", "x", "--max-cost-usd", "-1"]).is_err());
        assert!(parse(&["-p", "x", "--output-format", "yaml"]).is_err());
    }

    #[test]
    fn run_subcommand_still_owns_its_dir_flag() {
        let cli = parse(&["run", "-d", "/data"]).unwrap();
        let Some(Commands::Run(args)) = cli.command else {
            panic!("expected run");
        };
        assert_eq!(args.dir.as_deref(), Some(std::path::Path::new("/data")));
        assert!(cli.print.is_none());
    }
}
