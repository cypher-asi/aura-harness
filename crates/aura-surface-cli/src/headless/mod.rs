//! `aura -p`: run one prompt without the terminal UI, print the result,
//! and exit with a code that says how the run ended.
//!
//! It runs on the terminal's session machinery (agent registry, kernel
//! factory, turn driver, long-run settings, background jobs and session
//! checkpoints) as a dedicated "Headless" agent, so its sessions can be
//! resumed with `--resume` / `--continue` and opened in the terminal.
//! stdout carries only the chosen output format; logs go to stderr.

mod output;
mod run;
mod sink;
mod tools;

#[cfg(test)]
mod tests;

pub(crate) use output::{ExitReason, Output, RunReport};
pub(crate) use run::{Headless, Request, RunLimits, SessionChoice};
pub(crate) use tools::ToolFilter;

use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::cli::{HeadlessArgs, OutputFormat};
use crate::event_loop::TurnBase;
use crate::long_run::LongRunSettings;
use crate::model_selection::ModelSelection;
use crate::runner::{
    agent_tools, background_process_manager, build_hub, build_provider, mode_policy,
    policy_tool_names, post_edit_check_from_env, project_instructions, Harness, HubSetup,
};
use crate::session_helpers;
use anyhow::Context;
use aura_core_modes::AgentMode;
use aura_terminal::PermissionMode;
use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Default longest wait for background jobs after the agent's last turn
/// (`AURA_HEADLESS_JOB_WAIT`).
const DEFAULT_JOB_WAIT: Duration = Duration::from_secs(10 * 60);

/// Entry point for `aura -p`. `prompt` is the `-p` value (`-` reads
/// stdin).
///
/// # Errors
/// Only for problems before output starts in `text` format (a bad
/// `--cwd`, an unreadable stdin); `json` and `stream-json` report every
/// failure as a result object instead.
pub(crate) async fn run(
    prompt: String,
    args: HeadlessArgs,
    requested_mode: Option<AgentMode>,
) -> anyhow::Result<ExitCode> {
    init_tracing(args.verbose);
    let format = args.output_format.unwrap_or_default();
    let output = Output::new(format, Box::new(std::io::stdout()));

    let prepared = prepare(&prompt, &args, requested_mode).await;
    let (mut headless, request) = match prepared {
        Ok(prepared) => prepared,
        Err(e) if format == OutputFormat::Text => return Err(e),
        Err(e) => {
            let model = args.model.clone().unwrap_or_default();
            output.finish(&RunReport::failed(format!("{e:#}"), model));
            return Ok(ExitCode::from(ExitReason::Error.exit_code()));
        }
    };

    let interrupt = CancellationToken::new();
    let on_ctrl_c = interrupt.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            on_ctrl_c.cancel();
        }
    });

    let report = headless.execute(request, &output, &interrupt).await;
    output.finish(&report);
    if format == OutputFormat::Text {
        if let Some(error) = &report.error {
            eprintln!("aura: {error}");
        } else if report.is_error {
            eprintln!("aura: stopped ({:?})", report.exit_reason);
        }
    }
    Ok(ExitCode::from(report.exit_reason.exit_code()))
}

/// Logs go to stderr so stdout stays machine-readable. `RUST_LOG`
/// overrides the default of warnings only.
fn init_tracing(verbose: bool) {
    let mut filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    if verbose {
        if let Ok(directive) = "aura=debug".parse() {
            filter = filter.add_directive(directive);
        }
    }
    let _ = tracing_subscriber::registry()
        .with(fmt::layer().with_writer(std::io::stderr).with_ansi(false))
        .with(filter)
        .try_init();
}

/// Everything before the first turn: directory, prompt, limits, and the
/// agent harness.
async fn prepare(
    prompt: &str,
    args: &HeadlessArgs,
    requested_mode: Option<AgentMode>,
) -> anyhow::Result<(Headless, Request)> {
    if let Some(dir) = &args.cwd {
        // A one-shot process, so changing its directory is safe, and it
        // makes `./aura_data` and every relative path resolve the way the
        // terminal's do when started there.
        std::env::set_current_dir(dir)
            .with_context(|| format!("changing to --cwd {}", dir.display()))?;
    }
    let workspace_root = std::env::current_dir().context("resolving working directory")?;
    let prompt = read_prompt(prompt).await?;
    if prompt.trim().is_empty() {
        anyhow::bail!("no prompt: give one after -p or pipe it on stdin");
    }

    let long_run = LongRunSettings::from_env();
    let limits = RunLimits {
        max_turns: args.max_turns,
        max_time: args.max_time.or(long_run.turn_timeout),
        max_cost_usd: args.max_cost_usd.or(long_run.max_turn_cost_usd),
    };
    let session = match (&args.resume, args.continue_session) {
        (Some(id), _) => SessionChoice::Resume(id.clone()),
        (None, true) => SessionChoice::Continue,
        (None, false) => SessionChoice::New,
    };
    let data_dir = std::env::var("AURA_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace_root.join("aura_data"));
    let headless = build(args, requested_mode, workspace_root, &data_dir, long_run).await?;
    Ok((
        headless,
        Request {
            prompt,
            session,
            limits,
        },
    ))
}

/// The prompt from `arg`, or from stdin for `-`. Piped stdin is appended
/// to a prompt given on the command line.
async fn read_prompt(arg: &str) -> anyhow::Result<String> {
    let piped = !std::io::stdin().is_terminal();
    if arg == "-" && !piped {
        anyhow::bail!("-p without a prompt reads it from stdin, but stdin is a terminal");
    }
    if !piped {
        return Ok(arg.to_string());
    }
    let stdin = tokio::task::spawn_blocking(|| {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map(|_| text)
    })
    .await
    .context("reading stdin")?
    .context("reading stdin")?;
    Ok(match arg {
        "-" => stdin,
        _ if stdin.trim().is_empty() => arg.to_string(),
        _ => format!("{arg}\n\n{stdin}"),
    })
}

/// The agent harness `aura -p` runs on, built like the terminal's but with
/// no UI behind its command channel.
async fn build(
    args: &HeadlessArgs,
    requested_mode: Option<AgentMode>,
    workspace_root: PathBuf,
    data_dir: &std::path::Path,
    long_run: LongRunSettings,
) -> anyhow::Result<Headless> {
    tokio::fs::create_dir_all(data_dir)
        .await
        .with_context(|| format!("creating data directory {}", data_dir.display()))?;
    let registry = AgentRegistry::load_or_init(data_dir).context("loading agent registry")?;
    let definitions =
        AgentDefinitions::load_or_seed(data_dir).context("loading permanent agents")?;
    let store_path = session_helpers::resolve_store_path(data_dir);
    // RocksDB allows one process per store, so a terminal open on the
    // same data directory makes this fail rather than share it.
    let store = session_helpers::open_store(&store_path).with_context(|| {
        format!(
            "opening {} (is a terminal or another `aura -p` using this data directory?)",
            store_path.display()
        )
    })?;

    let (tool_config, tools, web_tools) = agent_tools(false);
    let tool_filter = ToolFilter::new(&args.allowed_tools, &args.disallowed_tools);
    let unknown = tool_filter.unknown(&tools);
    if !unknown.is_empty() {
        let known: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        warn!(
            ?unknown,
            ?known,
            "Tool names in --allowed-tools/--disallowed-tools match no tool"
        );
    }

    let provider_name = args.provider.as_deref().unwrap_or("anthropic");
    let mock = provider_name == "mock";
    let mut config = if mock {
        session_helpers::default_agent_config_with_auth(
            aura_model_reasoner::ENV_FALLBACK_MODEL,
            None,
        )
    } else {
        session_helpers::default_agent_config(aura_model_reasoner::ENV_FALLBACK_MODEL)
    };
    let (selection, auth) = if mock {
        // No sign-in state is needed, so tests and offline runs never
        // touch the keyring.
        (aura_model_reasoner::mock_provider(), None)
    } else {
        let auth =
            std::sync::Arc::new(aura_auth::AuthManager::new().context("loading sign-in state")?);
        (build_provider(provider_name, &auth)?, Some(auth))
    };
    let summary = auth
        .as_ref()
        .map_or_else(aura_auth::AuthSummary::default, |auth| {
            crate::event_loop::auth::auth_summary(auth, config.auth_token.as_deref())
        });
    let mut models = ModelSelection::init(&mut config, summary);
    if let Some(model) = &args.model {
        if let Err(problem) = models.use_for_run(&mut config, model) {
            if !mock {
                anyhow::bail!("--model {model}: {problem}");
            }
        }
    }
    if let Some(auth) = &auth {
        let local = |config: &aura_agent::AgentLoopConfig| {
            aura_model_reasoner::model_catalog::local_model_name(&config.model).is_some()
        };
        if models.is_gate() || local(&config) {
            models.refresh_local().await;
        }
        if models.is_gate() {
            if let Some(note) = models.refresh_plans(auth.as_ref()).await {
                warn!("{note}");
            }
            models.refresh_route(&mut config);
            info!(model = %config.model, "{}", models.display());
        }
        if local(&config) {
            if let Some(hint) = models.local_hint() {
                anyhow::bail!("{hint}");
            }
        }
    }
    let model = config.model.clone();

    let mode = aura_config::session_mode(requested_mode);
    let permission_mode = permission_mode(mode);
    let tool_names = policy_tool_names(&tools);
    let (commands, mut ui_commands) = mpsc::channel(256);
    // Spawned permanent agents report to a chat; nothing shows it here.
    tokio::spawn(async move { while ui_commands.recv().await.is_some() {} });
    let (process_manager, completions) = background_process_manager();
    let Harness { hub, background } = build_hub(HubSetup {
        data_dir: data_dir.to_path_buf(),
        workspace_root: workspace_root.clone(),
        registry,
        definitions,
        store,
        provider: selection.provider,
        tool_config,
        base: TurnBase {
            config,
            tools,
            limits: models.limits(),
        },
        policy: tool_filter.apply(mode_policy(permission_mode, &tool_names), &tool_names),
        record_observer: None,
        record_panel: crate::event_loop::RecordPanelAgent::default(),
        commands,
        long_run,
        process_manager,
        background_agents: true,
        mode: permission_mode,
        instructions: project_instructions(),
        approvals: None,
        web_tools,
        post_edit_check: post_edit_check_from_env(),
        trace_snapshots: false,
    });
    let notices = hub.take_notices();
    Ok(Headless {
        hub,
        background,
        completions,
        notices,
        model,
        mode,
        workspace_root,
        tool_filter,
        job_wait: job_wait_from_env(),
    })
}

/// The permission mode `aura -p` agents run in for the session mode
/// `mode` (`--mode`, else `AURA_DEFAULT_MODE`). `plan`, `ask` and `debug`
/// are the terminal's modes of those names. `agent` would start the
/// terminal in `default`, which asks before edits and commands; nobody
/// can answer here, so every ask is denied, and `agent` runs them instead.
/// `AURA_TUI_MODE` is the terminal's own starting mode and is not read.
pub(crate) const fn permission_mode(mode: AgentMode) -> PermissionMode {
    match PermissionMode::from_agent_mode(mode) {
        PermissionMode::Default => PermissionMode::Unrestricted,
        other => other,
    }
}

/// `AURA_HEADLESS_JOB_WAIT`, in the same forms as `--max-time`.
fn job_wait_from_env() -> Duration {
    std::env::var("AURA_HEADLESS_JOB_WAIT")
        .ok()
        .and_then(|raw| {
            let parsed = crate::long_run::parse_duration(&raw);
            if parsed.is_none() {
                warn!(value = %raw, "Ignoring unreadable AURA_HEADLESS_JOB_WAIT");
            }
            parsed
        })
        .unwrap_or(DEFAULT_JOB_WAIT)
}
