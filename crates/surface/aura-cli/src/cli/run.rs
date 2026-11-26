//! `aura run` and `aura desktop` arguments.

use aura_app::session::SessionOptions;
use clap::{Args, Parser, ValueEnum};
use std::path::PathBuf;

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
