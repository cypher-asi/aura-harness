//! Phase 10 carve-out 1: thin `aura` binary entrypoint.
//!
//! The desktop window has to own the main thread, so it opens here,
//! before any async runtime starts; everything else runs
//! `aura_surface_cli::run_cli` on a Tokio runtime.

use clap::Parser;
use std::process::ExitCode;

fn main() -> anyhow::Result<ExitCode> {
    let _ = dotenvy::dotenv();
    let cli = aura_surface_cli::cli::Cli::parse();

    #[cfg(feature = "desktop")]
    if let Some(desktop) = aura_surface_cli::desktop_launch(&cli) {
        aura_desktop::run(desktop, cli.mode)?;
        return Ok(ExitCode::SUCCESS);
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(aura_surface_cli::run_cli(cli))
}
