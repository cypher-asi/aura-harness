//! `aura-desktop`: the AURA terminal UI in a native, GPU-drawn window.
//!
//! Layer: surface
//!
//! The agent session runs in this process on the same event loop as the
//! terminal UI (`aura_surface_cli::session`), and the UI itself is the
//! terminal one (`aura_terminal::App`), drawn on a cell grid with Geist
//! Mono instead of through a terminal emulator.

mod app;
mod demo;
mod glyphs;
mod grid;
mod input;
mod perf;
mod router;
#[cfg(test)]
mod snapshot;
mod tab;
mod tab_bar;
mod theme;

use anyhow::Context;
use aura_surface_cli::cli::{DesktopArgs, RunArgs};
use aura_surface_cli::ModeFlag;
use std::time::Instant;
use tracing_subscriber::EnvFilter;

/// Open the desktop window with `args`, starting the agent session in
/// `mode` (or `AURA_DEFAULT_MODE`), and return once the window closes
/// and the session has shut down. Must run on the main thread.
///
/// # Errors
///
/// Fails when the workspace can't be opened, the session doesn't start,
/// or the window can't be created.
pub fn run(args: DesktopArgs, mode: Option<ModeFlag>) -> anyhow::Result<()> {
    let launched = Instant::now();
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

    let tabs_file = (!args.demo).then(|| router::arrangement_file(args.dir.as_deref()));
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
            mode.map(Into::into),
        ))?;
        let handles = app::Handles {
            events: session.events,
            commands: session.commands,
        };
        (handles, Some(session.task))
    };

    let options = app::Options {
        theme: args.theme,
        verbose: args.verbose,
        tabs_file,
    };
    let result = app::run(handles, options, launched);
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
