//! `aura login`, `aura logout` and `aura auth` arguments.

use clap::{Args, Subcommand};

/// Arguments for `aura login`.
#[derive(Args, Debug)]
pub struct LoginArgs {
    /// `zos` (default) for cloud models via aura-router, `claude` /
    /// `chatgpt` to use your own plan directly, or `openrouter` for
    /// models billed to your OpenRouter credits.
    #[arg(default_value = "zos")]
    pub provider: String,
    /// Paste the code shown by the provider instead of using a local
    /// browser redirect (for SSH and headless machines). For
    /// `openrouter`, an existing `sk-or-` API key may be pasted instead.
    #[arg(long)]
    pub paste: bool,
}

/// Arguments for `aura logout`.
#[derive(Args, Debug)]
pub struct LogoutArgs {
    /// `zos` (default) to sign out, or `claude` / `chatgpt` /
    /// `openrouter` to unlink.
    #[arg(default_value = "zos", conflicts_with = "all")]
    pub provider: String,
    /// Unlink every plan and sign out of zOS.
    #[arg(long)]
    pub all: bool,
}

/// `aura auth` parent command.
#[derive(Args, Debug)]
pub struct AuthCommand {
    #[command(subcommand)]
    pub action: AuthSubcommand,
}

/// Subcommands under `aura auth`.
#[derive(Subcommand, Debug)]
pub enum AuthSubcommand {
    /// Attach plans linked while signed out to the signed-in zOS account.
    Move {
        /// Move to the signed-in zOS account (required).
        #[arg(long, required = true)]
        to_zos: bool,
    },
}
