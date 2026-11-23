//! Thin `aura` binary entrypoint.
//!
//! The desktop window has to own the main thread, so it opens here,
//! before any async runtime starts; everything else runs
//! `aura_cli::run_cli` on a Tokio runtime, whose threads run below the
//! terminal UI's when there is one.

use clap::Parser;
use std::process::ExitCode;

fn main() -> anyhow::Result<ExitCode> {
    let _ = dotenvy::dotenv();
    let cli = aura_cli::cli::Cli::parse();

    #[cfg(feature = "desktop")]
    if let Some(desktop) = aura_cli::desktop_launch(&cli) {
        let options = aura_desktop::DesktopOptions {
            approval: cli.approval.map(Into::into),
            ..desktop_options(desktop)
        };
        aura_desktop::run(options, cli.mode.map(Into::into))?;
        return Ok(ExitCode::SUCCESS);
    }

    let mut runtime = tokio::runtime::Builder::new_multi_thread();
    runtime.enable_all();
    if let Some(start) = aura_cli::runtime_thread_start(&cli) {
        runtime.on_thread_start(start);
    }
    runtime.build()?.block_on(aura_cli::run_cli(cli))
}

#[cfg(feature = "desktop")]
fn desktop_options(args: aura_cli::cli::DesktopArgs) -> aura_desktop::DesktopOptions {
    aura_desktop::DesktopOptions {
        workspace: args.workspace,
        dir: args.dir,
        provider: args.provider,
        theme: args.theme,
        verbose: args.verbose,
        allow_unrestricted_full_access: args.allow_unrestricted_full_access,
        demo: args.demo,
        approval: None,
    }
}
