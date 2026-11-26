//! `aura cloud` and the hidden `aura node` subcommands.

use clap::{Args, Subcommand};

/// `aura cloud` parent command.
#[derive(Args, Debug)]
pub struct CloudCommand {
    /// Cloud subcommand to run.
    #[command(subcommand)]
    pub action: CloudSubcommand,
}

/// Subcommands under `aura cloud`.
#[derive(Subcommand, Debug)]
pub enum CloudSubcommand {
    /// Sign in to boxd and store an API key in `AURA_HOME/.env`.
    Setup,
    /// Show the cloud settings, where the key came from and the placed agents.
    Status,
}

/// `aura node` parent command.
#[derive(Args, Debug)]
pub struct NodeCommand {
    /// Node subcommand to run.
    #[command(subcommand)]
    pub action: NodeSubcommand,
}

/// Subcommands under `aura node`.
#[derive(Subcommand, Debug)]
pub enum NodeSubcommand {
    /// Hand this machine's node its hub's tokens (a `NodeClaim` on stdin).
    Claim {
        /// Read the claim from stdin (the only way it is accepted).
        #[arg(long)]
        stdin: bool,
    },
}
