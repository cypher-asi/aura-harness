//! `aura cloud setup|status` and the hidden `aura node claim --stdin`.

use std::io::Read;

use crate::cli::{CloudCommand, CloudSubcommand, NodeCommand, NodeSubcommand};
use anyhow::Context;

fn aura_home() -> anyhow::Result<std::path::PathBuf> {
    Ok(aura_config::AuraHome::resolve()
        .context("resolving AURA_HOME")?
        .path)
}

/// `aura cloud ...`.
///
/// # Errors
///
/// When setup fails or the settings can't be read.
pub(crate) async fn cmd_cloud(args: CloudCommand) -> anyhow::Result<()> {
    match args.action {
        CloudSubcommand::Setup => {
            let env_file = aura_home()?.join(".env");
            println!(
                "Signing in to boxd; the API key goes to {} (mode 600).",
                env_file.display()
            );
            let key = aura_app::cloud::setup::run(dirs::home_dir().as_deref(), &env_file)
                .await
                .context("cloud setup")?;
            println!(
                "Saved boxd key {key}. Use /cloud in the app to move an agent to its own machine."
            );
            Ok(())
        }
        CloudSubcommand::Status => {
            let config =
                aura_config::CloudConfig::reload().context("reading the cloud settings")?;
            match &config.credential {
                Some(credential) => println!("boxd credential: {credential}"),
                None => println!(
                    "boxd credential: none ({})",
                    aura_config::MISSING_CREDENTIAL_NOTICE
                ),
            }
            if let Some(report) = aura_config::env_file::report() {
                for file in report.loaded_files() {
                    println!("loaded: {}", file.display());
                }
                for warning in report.warnings() {
                    println!("warning: {warning}");
                }
            }
            println!("endpoint: {}", config.base_url());
            println!(
                "region: {}",
                config.region.as_deref().unwrap_or("fra (default)")
            );
            println!("machine size: {}", config.size);
            println!(
                "sleep: hibernate after {}s idle; isolated: {}; fork on spawn: {}",
                config.idle_hibernate_secs, config.isolated, config.fork_on_spawn
            );
            let placements = aura_home()?.join("cloud").join("placements.json");
            match std::fs::read_to_string(&placements) {
                Ok(text) => println!("placements ({}):\n{text}", placements.display()),
                Err(_) => println!("no agents placed yet"),
            }
            Ok(())
        }
    }
}

/// `aura node claim --stdin`: run inside a cloud machine by the hub.
///
/// # Errors
///
/// When the claim can't be read or the node refuses it.
pub(crate) async fn cmd_node(args: NodeCommand) -> anyhow::Result<()> {
    match args.action {
        NodeSubcommand::Claim { stdin } => {
            if !stdin {
                anyhow::bail!("the claim is only read from stdin: aura node claim --stdin");
            }
            let mut claim = Vec::new();
            std::io::stdin()
                .take(64 * 1024)
                .read_to_end(&mut claim)
                .context("reading the claim")?;
            let data_dir = aura_node::NodeConfig::from_env().data_dir;
            let socket = aura_node::cloud_node::claim_socket_path(&data_dir);
            let answer = aura_node::cloud_node::send_claim(&socket, &claim).await?;
            println!("{}", String::from_utf8_lossy(&answer));
            Ok(())
        }
    }
}
