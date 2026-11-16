//! Body of the `aura` CLI binary. The root `src/main.rs` initialises
//! `dotenvy` and calls [`run_cli`]; subcommands dispatch from here, and
//! the agent session itself runs in `aura-app`. The `aura-node`
//! entrypoint lives in `aura_runtime::run_node` and is re-exported from
//! this crate root.

use crate::auth_commands;
use crate::cli::{
    Cli, Commands, DesktopArgs, MigrateArgs, PluginsCommand, PluginsSubcommand, RunArgs, UiMode,
};

use anyhow::Context;
use aura_app::session;
use aura_terminal::{AlertSettings, App, Terminal, Theme};
use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

// ============================================================================
// Hello World (Spec 01)
// ============================================================================

/// Phase 10 surface-layer wrapper for the Spec 01 hello-world
/// banner. Kept public so external smoke tests can pin the
/// canonical message without depending on the binary crate.
#[must_use]
pub fn hello_world_message() -> &'static str {
    "Hello, World!"
}

/// Print the hello-world banner to stdout (Spec 01).
pub fn hello_world() {
    println!("{}", hello_world_message());
}

// ============================================================================
// Entry points
// ============================================================================

/// Surface-layer entry point for the `aura` binary: parses the CLI
/// arguments and runs [`run_cli`].
///
/// # Errors
///
/// Surfaces any error from the dispatched subcommand.
pub async fn run() -> anyhow::Result<std::process::ExitCode> {
    run_cli(Cli::parse()).await
}

/// The desktop window options when `cli` asks for the desktop app:
/// `aura desktop`, or `aura` alone unless `--terminal` is given or there
/// is no display to open a window on. The root binary opens the window
/// itself, since the window needs the main thread.
#[must_use]
pub fn desktop_launch(cli: &Cli) -> Option<DesktopArgs> {
    match &cli.command {
        Some(Commands::Desktop(args)) => Some(args.clone()),
        None if cli.print.is_none() && !cli.terminal && display_available() => {
            Some(DesktopArgs::default())
        }
        _ => None,
    }
}

/// Whether a window can open: always on macOS and Windows, and on other
/// systems when an X11 or Wayland display is set.
fn display_available() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
        || ["WAYLAND_DISPLAY", "DISPLAY"]
            .iter()
            .any(|var| std::env::var_os(var).is_some_and(|v| !v.is_empty()))
}

/// Dispatch parsed CLI arguments to the subcommand they name; with no
/// subcommand the agent runs in the terminal.
///
/// Subcommand errors are typed at their source (e.g.
/// `aura_auth::AuthError`, `aura_plugin_core::InstallError`) and bubble
/// up here as `anyhow::Error` because the root binary's `main` is the
/// only remaining `anyhow`-using boundary in the workspace.
///
/// # Errors
///
/// Surfaces any error from the dispatched subcommand, and refuses
/// `aura desktop` in a binary built without the desktop app.
pub async fn run_cli(cli: Cli) -> anyhow::Result<std::process::ExitCode> {
    let requested_mode = cli.mode.map(aura_core_modes::AgentMode::from);

    if let Some(prompt) = cli.print {
        if cli.command.is_some() {
            anyhow::bail!("-p runs a prompt on its own; it cannot be combined with a subcommand");
        }
        return aura_app::headless::run(prompt, cli.headless.into(), requested_mode).await;
    }

    match cli.command {
        Some(Commands::Desktop(_)) => Err(anyhow::anyhow!(
            "this aura was built without the desktop app; rebuild with `--features desktop` \
             or use `aura --terminal`"
        )),
        Some(Commands::Hello) => {
            hello_world();
            Ok(())
        }
        Some(Commands::Login(args)) => auth_commands::cmd_login(args).await,
        Some(Commands::Logout(args)) => auth_commands::cmd_logout(args).await,
        Some(Commands::Whoami) => auth_commands::cmd_whoami().await,
        Some(Commands::Auth(args)) => auth_commands::cmd_auth(args),
        Some(Commands::Migrate(args)) => cmd_migrate(args),
        Some(Commands::Plugins(args)) => cmd_plugins(args),
        Some(Commands::Runs(args)) => crate::trace_commands::cmd_runs(args).await,
        Some(Commands::Dataset(args)) => crate::trace_commands::cmd_dataset(args),
        Some(Commands::Eval(args)) => crate::trace_commands::cmd_eval(args).await,
        Some(Commands::Run(args)) => run_with_args(args, requested_mode).await,
        None => run_with_args(RunArgs::default(), requested_mode).await,
    }
    .map(|()| std::process::ExitCode::SUCCESS)
}

// ============================================================================
// Subcommand handlers
// ============================================================================

/// `aura migrate` (Phase 4a stub).
///
/// Intentionally a no-op today. Future phases will populate this
/// with the Codex → Aura state migration (config, credentials,
/// session history). Documented in the plan
/// (`phase-4a-config-aura-home`) so the CLI surface is stable when
/// later phases land the real migration logic.
fn cmd_migrate(args: MigrateArgs) -> anyhow::Result<()> {
    eprintln!(
        "aura migrate stub - Phase 4a placeholder; no migration actions performed (dry_run={})",
        args.dry_run
    );
    Ok(())
}

/// `aura plugins` (Phase 4b).
///
/// Dispatches to the install / list / enable / disable handlers.
/// Resolves `AURA_HOME` once and constructs a single
/// [`aura_plugin_core::PluginCache`] rooted at `AURA_HOME/plugins/`
/// before dispatching so every subcommand sees the same cache root
/// resolution rules.
fn cmd_plugins(args: PluginsCommand) -> anyhow::Result<()> {
    let aura_home = aura_config::AuraHome::resolve().context("resolving AURA_HOME")?;
    let cache = aura_plugin_core::PluginCache::new(aura_home.path.join("plugins"));
    match args.action {
        PluginsSubcommand::Install { source, trust } => {
            let installed = aura_plugin_core::install_with_trust(&source, &cache, trust)
                .with_context(|| format!("installing plugin from {}", source.display()))?;
            println!(
                "installed {} v{}",
                installed.manifest.id.as_str(),
                installed.manifest.version
            );
        }
        PluginsSubcommand::List => {
            let ids = cache.list_plugins().context("listing plugin cache")?;
            if ids.is_empty() {
                println!("(no plugins installed under {})", cache.root().display());
            }
            for id in ids {
                let active = cache
                    .active_version(&id)
                    .with_context(|| format!("reading active pointer for {id}"))?
                    .unwrap_or_else(|| "(none active)".to_string());
                println!("{id:<32} active={active}");
            }
        }
        PluginsSubcommand::Enable { id, yes, no } => {
            cmd_plugin_enable(&aura_home.path, &cache, &id, yes, no)?;
        }
        PluginsSubcommand::Disable { id } => {
            update_plugin_enable(&aura_home.path, &id, false, None)?;
            println!("disabled {id}");
        }
    }
    Ok(())
}

/// `aura plugins enable` flow with trust-prompt support.
///
/// Reads the prior `[plugins.<id>]` state from `config.toml`, hands
/// the cache + state to [`aura_plugin_core::enable_with_prompter`]
/// with the appropriate prompter (`AlwaysYes` for `--yes`,
/// `AlwaysNo` for `--no`, `TtyPrompter` otherwise), and writes the
/// resulting decision back to `config.toml`.
fn cmd_plugin_enable(
    aura_home: &std::path::Path,
    cache: &aura_plugin_core::PluginCache,
    id: &str,
    yes: bool,
    no: bool,
) -> anyhow::Result<()> {
    use aura_plugin_core::{
        enable_with_prompter, AlwaysNo, AlwaysYes, EnableDecision, TtyPrompter,
    };

    let prior = read_plugin_enable_state(aura_home, id)?;

    let outcome = if yes {
        enable_with_prompter(cache, id, prior, &mut AlwaysYes)
    } else if no {
        enable_with_prompter(cache, id, prior, &mut AlwaysNo)
    } else {
        enable_with_prompter(cache, id, prior, &mut TtyPrompter)
    }
    .with_context(|| format!("enabling plugin `{id}`"))?;

    match outcome.decision {
        EnableDecision::Enabled => {
            update_plugin_enable(
                aura_home,
                id,
                outcome.enabled_after,
                Some(outcome.trusted_after),
            )?;
            println!("enabled {id} (v{})", outcome.version);
        }
        EnableDecision::AlreadyTrusted => {
            update_plugin_enable(aura_home, id, true, Some(true))?;
            println!("enabled {id} (v{}; already trusted)", outcome.version);
        }
        EnableDecision::TrustDeclined => {
            println!("trust declined; plugin not enabled");
        }
    }
    Ok(())
}

/// Read `[plugins.<id>]` from `<aura_home>/config.toml`, returning a
/// snapshot of the `enabled` / `trusted` booleans. Returns the empty
/// state when the file or section is missing.
fn read_plugin_enable_state(
    aura_home: &std::path::Path,
    id: &str,
) -> anyhow::Result<aura_plugin_core::PluginEnableState> {
    use std::fs;

    let config_path = aura_home.join("config.toml");
    if !config_path.exists() {
        return Ok(aura_plugin_core::PluginEnableState::default());
    }
    let body = fs::read_to_string(&config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let root: toml::Value =
        toml::from_str(&body).with_context(|| format!("parsing {}", config_path.display()))?;
    let table = root
        .get("plugins")
        .and_then(|p| p.get(id))
        .and_then(|p| p.as_table());
    let enabled = table
        .and_then(|t| t.get("enabled"))
        .and_then(toml::Value::as_bool);
    let trusted = table
        .and_then(|t| t.get("trusted"))
        .and_then(toml::Value::as_bool);
    Ok(aura_plugin_core::PluginEnableState { enabled, trusted })
}

/// Minimal `~/.aura/config.toml` mutation: read (or synthesise) the
/// file, set `plugins.<id>.enabled = <enabled>`, atomic-write back
/// via tempfile + rename.
///
/// We intentionally use `toml::Value` (already a workspace dep) rather
/// than pulling in `toml_edit` for Phase 4b. The trade-off is that
/// comment / formatting is not preserved — acceptable today because
/// the config file is operator-owned and the round-trip writes a
/// consistent canonical layout.
fn update_plugin_enable(
    aura_home: &std::path::Path,
    id: &str,
    enabled: bool,
    trusted: Option<bool>,
) -> anyhow::Result<()> {
    use std::fs;

    let config_path = aura_home.join("config.toml");
    fs::create_dir_all(aura_home)
        .with_context(|| format!("creating AURA_HOME at {}", aura_home.display()))?;

    let mut root: toml::Value = if config_path.exists() {
        let body = fs::read_to_string(&config_path)
            .with_context(|| format!("reading {}", config_path.display()))?;
        toml::from_str(&body).with_context(|| format!("parsing {}", config_path.display()))?
    } else {
        toml::Value::Table(toml::value::Table::new())
    };

    let root_table = root
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config.toml root must be a table"))?;

    let plugins_entry = root_table
        .entry("plugins".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    let plugins_table = plugins_entry
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("[plugins] entry must be a table"))?;

    let plugin_entry = plugins_table
        .entry(id.to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    let plugin_table = plugin_entry
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("[plugins.{id}] entry must be a table"))?;

    plugin_table.insert("enabled".to_string(), toml::Value::Boolean(enabled));
    if let Some(t) = trusted {
        plugin_table.insert("trusted".to_string(), toml::Value::Boolean(t));
    }

    let serialised = toml::to_string_pretty(&root).context("serialising updated config.toml")?;

    let tmp = config_path.with_extension("toml.tmp");
    fs::write(&tmp, serialised).with_context(|| format!("writing {}", tmp.display()))?;
    if config_path.exists() {
        // Windows fs::rename refuses to overwrite; remove first.
        let _ = fs::remove_file(&config_path);
    }
    fs::rename(&tmp, &config_path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), config_path.display()))?;
    Ok(())
}

async fn run_with_args(
    args: RunArgs,
    requested_mode: Option<aura_core_modes::AgentMode>,
) -> anyhow::Result<()> {
    match args.ui {
        UiMode::Terminal => run_terminal(args, requested_mode).await,
        UiMode::None => {
            let filter = if args.verbose {
                EnvFilter::from_default_env().add_directive("aura=debug".parse()?)
            } else {
                EnvFilter::from_default_env().add_directive("aura=info".parse()?)
            };

            tracing_subscriber::registry()
                .with(
                    fmt::layer()
                        .event_format(aura_runtime::console_format::AuraConsoleFormat::new()),
                )
                .with(filter)
                .init();

            if let Some(mode) = requested_mode {
                warn!(
                    ?mode,
                    "--mode does not apply to the headless node; set the per-run `mode` \
                     field on POST /v1/run or AURA_DEFAULT_MODE instead"
                );
            }
            run_headless(args).await
        }
    }
}

// ============================================================================
// Terminal Mode
// ============================================================================

async fn run_terminal(
    args: RunArgs,
    requested_mode: Option<aura_core_modes::AgentMode>,
) -> anyhow::Result<()> {
    let theme = Theme::by_name(&args.theme);
    let session = session::start(&args.session_options(), requested_mode).await?;

    let mut app = App::new()
        .with_event_sender(session.events.clone())
        .with_command_receiver(session.commands)
        .with_alert_settings(AlertSettings::from_env());
    if args.verbose {
        app.set_verbose(true);
    }

    let mut terminal = Terminal::new(theme)?;
    let ran = terminal.run(&mut app);
    let finished = session.task.finish().await;
    ran?;
    finished
}

// ============================================================================
// Headless Mode (Node)
// ============================================================================

async fn run_headless(args: RunArgs) -> anyhow::Result<()> {
    info!("Starting AURA CLI in headless mode (node server)");

    let mut config = aura_runtime::NodeConfig::from_env();
    config.allow_unrestricted_full_access |= args.allow_unrestricted_full_access;

    aura_runtime::Node::new(config).run().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hello_world_message_exact() {
        assert_eq!(hello_world_message(), "Hello, World!");
    }

    #[test]
    fn test_hello_world_prints() {
        hello_world();
    }
}
