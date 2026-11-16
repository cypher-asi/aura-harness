//! `aura-desktop` binary: the desktop window without the rest of the
//! `aura` CLI.

use aura_surface_cli::cli::DesktopArgs;
use aura_surface_cli::ModeFlag;
use clap::Parser;

#[derive(Parser)]
#[command(name = "aura-desktop", about = "AURA in a native desktop window")]
struct Args {
    /// Session mode; overrides `AURA_DEFAULT_MODE`
    #[arg(long, value_enum)]
    mode: Option<ModeFlag>,

    #[command(flatten)]
    desktop: DesktopArgs,
}

fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let args = Args::parse();
    aura_desktop::run(args.desktop, args.mode)
}
