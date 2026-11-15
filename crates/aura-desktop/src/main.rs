//! `aura-desktop`: the AURA agent in a native, GPU-drawn window.
//!
//! The agent session runs in this process on the same event loop as the
//! terminal UI (`aura_surface_cli::session`); only the frontend differs.

mod ansi;
mod app;
mod demo;
mod draw;
mod model;
mod perf;
mod slash;
#[cfg(test)]
mod snapshot;
mod theme;
mod view;

use anyhow::Context;
use aura_surface_cli::cli::RunArgs;
use aura_surface_cli::ModeFlag;
use clap::Parser;
use std::path::PathBuf;
use std::time::Instant;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "aura-desktop", about = "AURA in a native desktop window")]
struct Args {
    /// Folder the agent works in (default: the current directory)
    #[arg(short, long)]
    workspace: Option<PathBuf>,

    /// Data directory for agents and records (default: `AURA_DATA_DIR` or `./aura_data`)
    #[arg(short, long)]
    dir: Option<PathBuf>,

    /// Model provider: `anthropic` (local Ollama models + aura-router) or `mock`
    #[arg(long, default_value = "anthropic")]
    provider: String,

    /// Session mode; overrides `AURA_DEFAULT_MODE`
    #[arg(long, value_enum)]
    mode: Option<ModeFlag>,

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

fn main() -> anyhow::Result<()> {
    let launched = Instant::now();
    let _ = dotenvy::dotenv();
    let args = Args::parse();
    init_tracing(args.verbose);

    if let Some(workspace) = &args.workspace {
        std::env::set_current_dir(workspace)
            .with_context(|| format!("opening workspace {}", workspace.display()))?;
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("aura-agent")
        .build()
        .context("starting the async runtime")?;
    // Widgets that spawn futures (e.g. file reads) run on this runtime too.
    let _guard = runtime.enter();

    let (handles, task) = if args.demo {
        (demo::start(&runtime), None)
    } else {
        let run_args = RunArgs {
            dir: args.dir,
            provider: args.provider,
            verbose: args.verbose,
            allow_unrestricted_full_access: args.allow_unrestricted_full_access,
            ..RunArgs::default()
        };
        let session = runtime.block_on(aura_surface_cli::session::start(
            &run_args,
            args.mode.map(Into::into),
        ))?;
        let handles = app::Handles {
            events: session.events,
            commands: session.commands,
        };
        (handles, Some(session.task))
    };

    let result = app::run(handles, launched);
    if let Some(task) = task {
        runtime.block_on(task.finish())?;
    }
    result.context("desktop window failed")
}

fn init_tracing(verbose: bool) {
    let default = if verbose {
        "aura=debug,aura_desktop=debug"
    } else {
        "aura=info,aura_desktop=info,wgpu=warn,iced=warn"
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
