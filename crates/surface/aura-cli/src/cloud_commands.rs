//! `aura cloud setup|status` and the hidden `aura node claim --stdin|protocol`.

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
        CloudSubcommand::Setup => cloud_setup().await,
        CloudSubcommand::Status => cloud_status(),
    }
}

/// `aura cloud setup`: sign in to boxd and save the key.
async fn cloud_setup() -> anyhow::Result<()> {
    let env_file = aura_home()?.join(".env");
    println!(
        "Signing in to boxd; the API key goes to {} (mode 600).",
        env_file.display()
    );
    let key = aura_app::cloud::setup::run(dirs::home_dir().as_deref(), &env_file)
        .await
        .context("cloud setup")?;
    println!("Saved boxd key {key}. Use /cloud in the app to move an agent to its own machine.");
    Ok(())
}

/// `aura cloud status`: the settings, where the key came from and the
/// placed agents.
fn cloud_status() -> anyhow::Result<()> {
    let config = aura_config::CloudConfig::reload().context("reading the cloud settings")?;
    print_credential(&config);
    print_env_report();
    print_settings(&config);
    let placements = aura_home()?.join("cloud").join("placements.json");
    match std::fs::read_to_string(&placements) {
        Ok(text) => print_owners(&text),
        Err(_) => println!("no agents placed yet"),
    }
    Ok(())
}

fn print_credential(config: &aura_config::CloudConfig) {
    match &config.credential {
        Some(credential) => println!("boxd credential: {credential}"),
        None => println!(
            "boxd credential: none ({})",
            aura_config::MISSING_CREDENTIAL_NOTICE
        ),
    }
}

/// The `.env` files loaded at startup and any warnings about them.
fn print_env_report() {
    if let Some(report) = aura_config::env_file::report() {
        for file in report.loaded_files() {
            println!("loaded: {}", file.display());
        }
        for warning in report.warnings() {
            println!("warning: {warning}");
        }
    }
}

fn print_settings(config: &aura_config::CloudConfig) {
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
}

/// `aura node claim --stdin` and `aura node protocol`: run inside a cloud
/// machine by the hub.
///
/// # Errors
///
/// When the claim can't be read or the node refuses it.
pub(crate) async fn cmd_node(args: NodeCommand) -> anyhow::Result<()> {
    match args.action {
        NodeSubcommand::Protocol => {
            println!("{}", aura_core_protocol::cloud::NODE_PROTOCOL);
            Ok(())
        }
        NodeSubcommand::Claim { stdin } => {
            if !stdin {
                anyhow::bail!("the claim is only read from stdin: aura node claim --stdin");
            }
            let mut claim = Vec::new();
            std::io::stdin()
                .take(64 * 1024)
                .read_to_end(&mut claim)
                .context("reading the claim")?;
            let mut parsed: aura_core_protocol::cloud::NodeClaim =
                serde_json::from_slice(&claim).context("parsing the claim")?;
            let filled = aura_node::cloud_node::fill_placeholders(&mut parsed, placeholder)
                .map_err(anyhow::Error::msg)?;
            eprintln!(
                "plans with a placeholder: {filled} of {}",
                parsed.plans.len()
            );
            if let Some(git) = &parsed.git {
                eprintln!(
                    "git: branch {}, GitHub token {}",
                    git.branch,
                    if git.placeholder.is_some() {
                        "placeholder set"
                    } else {
                        "none"
                    }
                );
            }
            let claim = serde_json::to_vec(&parsed).context("encoding the claim")?;
            let data_dir = aura_node::NodeConfig::from_env().data_dir;
            let socket = aura_node::cloud_node::claim_socket_path(&data_dir);
            let answer = aura_node::cloud_node::send_claim(&socket, &claim).await?;
            println!("{}", String::from_utf8_lossy(&answer));
            Ok(())
        }
    }
}

/// A boxd secret's placeholder: from this process's environment, or a
/// login shell's (boxd exports secrets in login shells; a value added
/// after boot shows up there first).
fn placeholder(name: &str) -> Option<String> {
    if let Ok(value) = std::env::var(name) {
        return Some(value);
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let output = std::process::Command::new("bash")
        .args(["-lc", &format!("printf %s \"${{{name}:-}}\"")])
        .output()
        .ok()?;
    let value = String::from_utf8(output.stdout).ok()?;
    (!value.is_empty()).then_some(value)
}

/// Each user with placed agents, by display name, and their machines.
fn print_owners(placements: &str) {
    let Ok(file) = serde_json::from_str::<serde_json::Value>(placements) else {
        println!("placements file is unreadable");
        return;
    };
    let mut owners: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    if let Some(agents) = file.get("agents").and_then(|a| a.as_object()) {
        for placed in agents.values() {
            let name = placed
                .get("owner_name")
                .and_then(|n| n.as_str())
                .map_or_else(
                    || match placed
                        .get("owner")
                        .and_then(|o| o.get("kind"))
                        .and_then(|k| k.as_str())
                    {
                        Some("zos") => "a zOS user".to_string(),
                        _ => "local profile".to_string(),
                    },
                    str::to_string,
                );
            let machine = placed
                .get("machine")
                .and_then(|m| m.as_str())
                .unwrap_or("?")
                .to_string();
            owners.entry(name).or_default().push(machine);
        }
    }
    if owners.is_empty() {
        println!("no agents placed yet");
    }
    for (owner, machines) in owners {
        println!(
            "{owner}: {} machine{} ({})",
            machines.len(),
            if machines.len() == 1 { "" } else { "s" },
            machines.join(", ")
        );
    }
}
