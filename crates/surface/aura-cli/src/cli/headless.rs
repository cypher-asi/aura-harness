//! Options for `aura -p` and how it prints its result.

use aura_app::headless::HeadlessOptions;
use clap::{Args, ValueEnum};
use std::path::PathBuf;
use std::time::Duration;

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
