//! CLI argument definitions and parsing.

mod auth;
mod changelog;
mod cloud;
mod eval;
mod gate;
mod headless;
mod plugins;
mod run;
mod runs;

pub use auth::{AuthCommand, AuthSubcommand, LoginArgs, LogoutArgs};
pub use changelog::ChangelogArgs;
pub use cloud::{CloudCommand, CloudSubcommand, NodeCommand, NodeSubcommand};
pub use eval::{DatasetCommand, DatasetSubcommand, EvalCommand, EvalSubcommand};
pub use gate::{GateCommand, GateSubcommand};
pub use headless::{HeadlessArgs, OutputFormat};
pub use plugins::{MigrateArgs, PluginsCommand, PluginsSubcommand};
pub use run::{DesktopArgs, RunArgs, UiMode};
pub use runs::{HarnessArgs, RunsCommand, RunsSubcommand};

use crate::{ApprovalFlag, ModeFlag};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "aura",
    about = "AURA CLI - Autonomous Universal Reasoning Architecture"
)]
pub struct Cli {
    /// Session mode for the desktop window or terminal. Overrides the daemon
    /// default (`AURA_DEFAULT_MODE`); the kernel denies any tool call the
    /// mode does not allow.
    #[arg(long = "mode", value_enum, global = true)]
    pub mode: Option<ModeFlag>,

    /// What tool calls ask first: `yolo` runs everything with commands
    /// confined to writing inside the working folder, `ask` asks for
    /// anything not on the allow list, `strict` asks for everything but
    /// reads. Overrides `AURA_TUI_APPROVAL`; `-p` defaults to `yolo` in
    /// agent mode.
    #[arg(long = "approval", value_enum, global = true)]
    pub approval: Option<ApprovalFlag>,

    /// Run one prompt without the terminal UI, print the result, and exit.
    /// `-p -`, or `-p` with no prompt, reads the prompt from stdin; piped
    /// stdin is appended to a prompt given here.
    #[arg(
        short = 'p',
        long = "print",
        value_name = "PROMPT",
        num_args = 0..=1,
        default_missing_value = "-"
    )]
    pub print: Option<String>,

    /// Run the agent in this terminal instead of the desktop window.
    /// Without a display (e.g. over SSH) the terminal is used anyway.
    #[arg(long, visible_alias = "tui", conflicts_with = "print")]
    pub terminal: bool,

    #[command(flatten)]
    pub headless: HeadlessArgs,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run the agent in the desktop window (default with no subcommand).
    Desktop(DesktopArgs),
    /// Run the agent in the terminal, or as a server with `--ui none`.
    Run(RunArgs),
    /// Sign in to zOS, or link a Claude / ChatGPT plan.
    Login(LoginArgs),
    /// Sign out of zOS, or unlink a plan.
    Logout(LogoutArgs),
    /// Show the zOS sign-in and linked plans.
    Whoami,
    /// Manage linked plans.
    Auth(AuthCommand),
    /// Print "Hello, World!" and exit (Spec 01).
    Hello,
    /// Phase 4a stub: migrate aura state (no-op today).
    ///
    /// Future phases will populate this with Codex → Aura state
    /// migration. The stub exists so the CLI surface is stable when
    /// `aura-store-db` and `aura-plugin` (Phase 4b+) start
    /// producing migration-aware on-disk layouts.
    Migrate(MigrateArgs),
    /// Manage declarative plugins (Phase 4b).
    ///
    /// Subcommands install / list / enable / disable interact with
    /// the on-disk plugin cache under `AURA_HOME/plugins/` and the
    /// `[plugins.<id>]` table inside `AURA_HOME/config.toml`. No
    /// agent-loop wiring lands until Phase 4c (hooks/MCP/connectors)
    /// and Phase 8 (full integration).
    Plugins(PluginsCommand),
    /// Inspect, compare and label recorded runs (`AURA_HOME/runs/`).
    Runs(RunsCommand),
    /// Export recorded runs as training data.
    Dataset(DatasetCommand),
    /// Run coding eval suites and compare harness arms.
    Eval(EvalCommand),
    /// Check Gate's verdicts against what happened.
    Gate(GateCommand),
    /// Run agents on their own boxd cloud machines.
    Cloud(CloudCommand),
    /// Show the `changelog.d/` fragments as CHANGELOG.md entries;
    /// `--write` puts them in, for a release.
    Changelog(ChangelogArgs),
    /// What a cloud machine's node runs for its hub, over boxd's `Exec`.
    #[command(hide = true)]
    Node(NodeCommand),
}

#[cfg(test)]
mod tests;
