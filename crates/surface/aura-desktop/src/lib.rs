//! `aura-desktop`: the AURA terminal UI in a native, GPU-drawn window.
//!
//! Layer: surface
//!
//! The agent session runs in this process on the same event loop as the
//! terminal UI (`aura_app::session`), and the UI itself is the
//! terminal one (`aura_ui_terminal::App`), drawn on a cell grid with Geist
//! Mono instead of through a terminal emulator.
//!
//! The window is winit's and has no toolkit in it. Each frame the tabs
//! of terminal UI become one list of quads (`paint`), which the GPU
//! draws in one call (`gpu`), or the CPU where there is none (`raster`).
//! `shell` holds what the window shows and decides when a frame is due.

mod canvas;
mod chrome;
mod demo;
mod geom;
mod gpu;
mod grid;
mod paint;
mod raster;
mod router;
mod scene;
mod shapes;
mod shell;
mod tab;
mod tab_bar;
mod text;
mod theme;

use anyhow::Context;
use aura_app::session::SessionOptions;
use aura_core_modes::{AgentMode, ApprovalLevel};
use std::path::PathBuf;
use std::time::Instant;
use tracing_subscriber::EnvFilter;

/// Settings for the desktop window and the session behind it.
#[derive(Debug, Clone)]
pub struct DesktopOptions {
    /// Folder the agent works in (default: the current directory)
    pub workspace: Option<PathBuf>,
    /// Data directory for agents and records (default: `AURA_DATA_DIR` or
    /// `./aura_data`)
    pub dir: Option<PathBuf>,
    /// Model provider: `anthropic` (local Ollama models + aura-router) or
    /// `mock`
    pub provider: String,
    /// Theme (cyber, matrix, synthwave, minimal)
    pub theme: String,
    /// Log debug output to stderr
    pub verbose: bool,
    /// Permit FullAccess sessions to bypass command allowlists
    pub allow_unrestricted_full_access: bool,
    /// Run against a scripted session instead of a real agent
    pub demo: bool,
    /// Approval level agents start at; `None` reads `AURA_TUI_APPROVAL`,
    /// else `yolo`
    pub approval: Option<ApprovalLevel>,
}

impl Default for DesktopOptions {
    fn default() -> Self {
        Self {
            workspace: None,
            dir: None,
            provider: "anthropic".to_string(),
            theme: "cyber".to_string(),
            verbose: false,
            allow_unrestricted_full_access: false,
            demo: false,
            approval: None,
        }
    }
}

/// Open the desktop window with `args`, starting the agent session in
/// `mode` (or `AURA_DEFAULT_MODE`), and return once the window closes
/// and the session has shut down. Must run on the main thread.
///
/// # Errors
///
/// Fails when the workspace can't be opened, the session doesn't start,
/// or the window can't be created.
pub fn run(args: DesktopOptions, mode: Option<AgentMode>) -> anyhow::Result<()> {
    let launched = Instant::now();
    init_tracing(args.verbose);

    if let Some(workspace) = &args.workspace {
        std::env::set_current_dir(workspace)
            .with_context(|| format!("opening workspace {}", workspace.display()))?;
    }

    // The agents run here, below the window in priority; the window
    // takes commands and starts editors from threads of its own.
    let budget = aura_app::threads::Budget::of_machine();
    let runtime = budget
        .runtime()
        .on_thread_start(aura_app::priority::agent_thread_start())
        .build()
        .context("starting the async runtime")?;
    budget.start_compute_pool(aura_app::priority::agent_nice());
    let _guard = runtime.enter();

    let tabs_file = (!args.demo).then(|| router::arrangement_file(args.dir.as_deref()));
    let (handles, task) = if args.demo {
        (demo::start(&runtime), None)
    } else {
        let options = SessionOptions {
            dir: args.dir,
            provider: args.provider,
            allow_unrestricted_full_access: args.allow_unrestricted_full_access,
            approval: args.approval,
        };
        let session = runtime.block_on(aura_app::session::start(&options, mode))?;
        let handles = shell::Handles {
            events: session.events,
            commands: session.commands,
        };
        (handles, Some(session.task))
    };

    let options = shell::Options {
        theme: args.theme,
        verbose: args.verbose,
        tabs_file,
    };
    let result = shell::run(handles, &options, launched);
    if let Some(task) = task {
        runtime.block_on(task.finish())?;
    }
    result.context("desktop window failed")
}

fn init_tracing(verbose: bool) {
    let default = if verbose {
        "aura=debug,aura_desktop=debug"
    } else {
        "aura=info,aura_desktop=info,wgpu=warn"
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
