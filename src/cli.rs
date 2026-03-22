//! CLI argument definitions and parsing.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "aura",
    about = "AURA CLI - Autonomous Universal Reasoning Architecture"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Run the agent (default when no subcommand is given).
    Run(RunArgs),
    /// Authenticate with zOS to obtain a JWT for proxy mode.
    Login,
    /// Clear stored authentication credentials.
    Logout,
    /// Show current authentication status.
    Whoami,
}

/// Arguments for the `run` subcommand (also the default behaviour).
#[derive(Parser)]
pub(crate) struct RunArgs {
    /// UI mode (terminal or none)
    #[arg(long, default_value = "terminal")]
    pub ui: UiMode,

    /// Theme (cyber, matrix, synthwave, minimal)
    #[arg(long, default_value = "cyber")]
    pub theme: String,

    /// Working directory
    #[arg(short, long)]
    pub dir: Option<PathBuf>,

    /// Model provider (anthropic or mock)
    #[arg(long, default_value = "anthropic")]
    pub provider: String,

    /// Enable verbose output
    #[arg(short, long)]
    pub verbose: bool,
}

impl Default for RunArgs {
    fn default() -> Self {
        Self {
            ui: UiMode::Terminal,
            theme: "cyber".to_string(),
            dir: None,
            provider: "anthropic".to_string(),
            verbose: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum UiMode {
    /// Full terminal UI (default)
    Terminal,
    /// No UI, run as swarm server
    None,
}
