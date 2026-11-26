//! `aura migrate` and `aura plugins` arguments.

use clap::{Args, Subcommand};
use std::path::PathBuf;

/// Arguments for the `migrate` subcommand (Phase 4a stub).
#[derive(Args, Debug)]
pub struct MigrateArgs {
    /// Run as a dry preview without making changes.
    #[arg(long)]
    pub dry_run: bool,
}

/// `aura plugins` parent command (Phase 4b).
#[derive(Args, Debug)]
pub struct PluginsCommand {
    /// Plugins subcommand to run.
    #[command(subcommand)]
    pub action: PluginsSubcommand,
}

/// Subcommands under `aura plugins` (Phase 4b).
#[derive(Subcommand, Debug)]
pub enum PluginsSubcommand {
    /// Install a plugin from a local source directory.
    ///
    /// The source must contain a `.aura-plugin/`, `.codex-plugin/`,
    /// or `.claude-plugin/` subdirectory with a `manifest.toml`. The
    /// install pipeline copies the source tree into the
    /// `AURA_HOME/plugins/<id>/<version>/` cache layout and writes a
    /// normalised `.aura-plugin.toml` regardless of which alias the
    /// source used.
    Install {
        /// Path to the plugin source directory.
        source: PathBuf,
        /// Bypass `trust.require_explicit_trust = true` manifests.
        #[arg(long)]
        trust: bool,
    },
    /// List installed plugins and their active versions.
    List,
    /// Enable a plugin in `AURA_HOME/config.toml` (`enabled = true`).
    ///
    /// When the cached manifest has `trust.require_explicit_trust = true`
    /// and the plugin is not already trusted in operator config, the
    /// flow prompts the operator on the TTY. `--yes` skips the
    /// prompt and accepts trust; `--no` skips the prompt and
    /// declines (the plugin is NOT enabled).
    Enable {
        /// Plugin id (matches the manifest `id` field).
        id: String,
        /// Bypass the trust prompt and accept trust automatically.
        /// Mutually exclusive with `--no`.
        #[arg(long, conflicts_with = "no")]
        yes: bool,
        /// Bypass the trust prompt and decline trust automatically.
        /// Mutually exclusive with `--yes`.
        #[arg(long, conflicts_with = "yes")]
        no: bool,
    },
    /// Disable a plugin in `AURA_HOME/config.toml` (`enabled = false`).
    Disable {
        /// Plugin id (matches the manifest `id` field).
        id: String,
    },
}
