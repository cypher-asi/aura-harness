//! `aura-desktop` binary: the desktop window without the rest of the
//! `aura` CLI.

use aura_core_modes::AgentMode;
use aura_desktop::DesktopOptions;
use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "aura-desktop", about = "AURA in a native desktop window")]
struct Args {
    /// Session mode; overrides `AURA_DEFAULT_MODE`
    #[arg(long, value_enum)]
    mode: Option<Mode>,

    /// Folder the agent works in (default: the current directory)
    #[arg(short, long)]
    workspace: Option<PathBuf>,

    /// Data directory for agents and records (default: `AURA_DATA_DIR` or `./aura_data`)
    #[arg(short, long)]
    dir: Option<PathBuf>,

    /// Model provider: `anthropic` (local Ollama models + aura-router) or `mock`
    #[arg(long, default_value = "anthropic")]
    provider: String,

    /// Theme (cyber, matrix, synthwave, minimal)
    #[arg(long, default_value = "cyber")]
    theme: String,

    /// Log debug output to stderr
    #[arg(short, long)]
    verbose: bool,

    /// Permit FullAccess sessions to bypass command allowlists
    #[arg(long)]
    allow_unrestricted_full_access: bool,

    /// Run against a scripted session instead of a real agent
    #[arg(long)]
    demo: bool,
}

#[derive(Clone, Copy, ValueEnum)]
#[clap(rename_all = "lower")]
enum Mode {
    Agent,
    Plan,
    Ask,
    Debug,
}

impl From<Mode> for AgentMode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Agent => Self::Agent,
            Mode::Plan => Self::Plan,
            Mode::Ask => Self::Ask,
            Mode::Debug => Self::Debug,
        }
    }
}

fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let args = Args::parse();
    let options = DesktopOptions {
        workspace: args.workspace,
        dir: args.dir,
        provider: args.provider,
        theme: args.theme,
        verbose: args.verbose,
        allow_unrestricted_full_access: args.allow_unrestricted_full_access,
        demo: args.demo,
    };
    aura_desktop::run(options, args.mode.map(Into::into))
}
