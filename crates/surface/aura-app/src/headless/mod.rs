//! `aura -p`: run one prompt without the terminal UI, print the result,
//! and exit with a code that says how the run ended.
//!
//! It runs on the terminal's session machinery (agent registry, kernel
//! factory, turn driver, long-run settings, background jobs and session
//! checkpoints) as a dedicated "Headless" agent, so its sessions can be
//! resumed with `--resume` / `--continue` and opened in the terminal.
//! stdout carries only the chosen output format; logs go to stderr.

mod eval;
mod gate_replay;
mod output;
mod run;
mod sink;
mod tools;

#[cfg(test)]
mod tests;

pub use crate::event_loop::task_check::CheckRun;
pub use eval::{copy_dir, EvalRan, EvalTurn, EvalTurns, GateUse};
pub use gate_replay::{gate_replay, ReplayOptions};
pub(crate) use output::{ExitReason, Output, RunReport};
pub use run::Headless;
pub(crate) use run::{Request, RunLimits, SessionChoice};
pub(crate) use tools::ToolFilter;

use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::AgentRegistry;
use crate::event_loop::Permissions;
use crate::event_loop::TurnBase;
use crate::harness::{
    agent_tools, background_process_manager, build_hub, build_provider, mode_policy,
    policy_tool_names, post_edit_check_from_env, project_instructions, Harness, HubSetup,
};
use crate::long_run::LongRunSettings;
use crate::model_selection::{may_be_alias, ModelSelection};
use crate::session_helpers;
use anyhow::Context;
use aura_core_modes::AgentMode;
use aura_ui_terminal::{ApprovalLevel, PermissionMode};
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

/// Settings for one `aura -p` run.
#[derive(Debug, Clone, Default)]
pub struct HeadlessOptions {
    /// What stdout carries
    pub output_format: OutputFormat,
    /// Continue the saved session with this id
    pub resume: Option<String>,
    /// Continue the most recent `aura -p` session
    pub continue_session: bool,
    /// Only these tools may run; empty allows every tool
    pub allowed_tools: Vec<String>,
    /// These tools may not run
    pub disallowed_tools: Vec<String>,
    /// Stop after this many model calls
    pub max_turns: Option<u32>,
    /// Stop after this long; overrides `AURA_TUI_TURN_TIMEOUT`
    pub max_time: Option<Duration>,
    /// Stop once model calls cost more than this many US dollars;
    /// overrides `AURA_TUI_MAX_TURN_COST_USD`
    pub max_cost_usd: Option<f64>,
    /// Work in this directory instead of the current one
    pub cwd: Option<PathBuf>,
    /// Model for this run, not saved as the `/model` choice
    pub model: Option<String>,
    /// Model provider: `anthropic` (the default) or `mock`
    pub provider: Option<String>,
    /// Log debug detail to stderr
    pub verbose: bool,
    /// Approval level (`--approval`); `None` runs `agent` mode at `yolo`
    /// and the other modes at `ask`
    pub approval: Option<ApprovalLevel>,
}

/// How `aura -p` prints its result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputFormat {
    /// The final reply as plain text.
    #[default]
    Text,
    /// One JSON object describing the run.
    Json,
    /// Newline-delimited JSON events, ending with the result object.
    StreamJson,
}

/// Entry point for `aura -p`. `prompt` is the `-p` value (`-` reads
/// stdin).
///
/// # Errors
/// Only for problems before output starts in `text` format (a bad
/// `--cwd`, an unreadable stdin); `json` and `stream-json` report every
/// failure as a result object instead.
pub async fn run(
    prompt: String,
    args: HeadlessOptions,
    requested_mode: Option<AgentMode>,
) -> anyhow::Result<ExitCode> {
    init_tracing(args.verbose);
    let format = args.output_format;
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
    args: &HeadlessOptions,
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
        max_iterations: None,
    };
    let session = match (&args.resume, args.continue_session) {
        (Some(id), _) => SessionChoice::Resume(id.clone()),
        (None, true) => SessionChoice::Continue,
        (None, false) => SessionChoice::New,
    };
    let data_dir = data_dir(&workspace_root);
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

/// Where the terminal started in `workspace_root` keeps its agents and
/// records: `AURA_DATA_DIR`, else `aura_data` there.
fn data_dir(workspace_root: &std::path::Path) -> PathBuf {
    std::env::var("AURA_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace_root.join("aura_data"))
}

/// The harness `aura eval run` runs tasks on under `gate`: `aura -p`'s
/// with `provider`, its agents in `agent` mode at `yolo` (nobody answers
/// a prompt in an unattended run; commands stay confined to the task's
/// workspace), on the agents and records of the terminal started in the
/// current directory, so the runs count in that Gate table.
///
/// # Errors
/// When the directory, sign-in state or data directory can't be read.
pub async fn eval_harness(provider: &str) -> anyhow::Result<Headless> {
    let workspace_root = std::env::current_dir().context("resolving working directory")?;
    let data_dir = data_dir(&workspace_root);
    let args = HeadlessOptions {
        model: Some(aura_model::model_catalog::GATE_MODEL_ID.to_string()),
        provider: Some(provider.to_string()),
        approval: Some(ApprovalLevel::Yolo),
        ..HeadlessOptions::default()
    };
    build(
        &args,
        Some(AgentMode::Agent),
        workspace_root,
        &data_dir,
        LongRunSettings::from_env(),
    )
    .await
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
    args: &HeadlessOptions,
    requested_mode: Option<AgentMode>,
    workspace_root: PathBuf,
    data_dir: &std::path::Path,
    long_run: LongRunSettings,
) -> anyhow::Result<Headless> {
    tokio::fs::create_dir_all(data_dir)
        .await
        .with_context(|| format!("creating data directory {}", data_dir.display()))?;
    let registry = AgentRegistry::load_or_init_in(data_dir, &workspace_root)
        .context("loading agent registry")?;
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
        session_helpers::default_agent_config_with_auth(aura_model::ENV_FALLBACK_MODEL, None)
    } else {
        session_helpers::default_agent_config(aura_model::ENV_FALLBACK_MODEL)
    };
    let (selection, auth) = if mock {
        // No sign-in state is needed, so tests and offline runs never
        // touch the keyring.
        (aura_model::mock_provider(), None)
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
    let mut notes = Vec::new();
    // A short name (`opus`, `deepseek`) resolves among the listed models,
    // so load the live lists first. Mock runs skip this: they have no
    // sign-ins and must not touch the network.
    let mut local_probed = false;
    let mut plans_refreshed = false;
    if let (Some(model), Some(auth)) = (&args.model, &auth) {
        if may_be_alias(model) {
            models.refresh_local().await;
            local_probed = true;
            if let Some(note) = models.refresh_plans(auth.as_ref()).await {
                warn!("{note}");
                notes.push(note);
            }
            plans_refreshed = true;
        }
    }
    if let Some(model) = &args.model {
        if let Err(problem) = models.use_for_run(&mut config, model) {
            if !mock {
                anyhow::bail!("--model {model}: {problem}");
            }
        }
    }
    if let Some(auth) = &auth {
        let local = |config: &aura_agent::AgentLoopConfig| {
            aura_model::model_catalog::local_model_name(&config.model).is_some()
        };
        if !local_probed && (models.is_gate() || local(&config)) {
            models.refresh_local().await;
        }
        if models.is_gate() {
            if !plans_refreshed {
                if let Some(note) = models.refresh_plans(auth.as_ref()).await {
                    warn!("{note}");
                    notes.push(note);
                }
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
    let permissions = permissions(mode, args.approval);
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
            gate: crate::event_loop::GateBase::of(&models),
        },
        gate: aura_config::gate(),
        policy: tool_filter.apply(mode_policy(permissions, &tool_names), &tool_names),
        record_observer: None,
        record_panel: crate::event_loop::RecordPanelAgent::default(),
        record_router: None,
        trace_home: aura_config::AuraHome::resolve().ok().map(|home| home.path),
        commands,
        long_run,
        process_manager,
        background_agents: true,
        permissions,
        instructions: project_instructions(),
        approvals: None,
        web_tools,
        post_edit_check: post_edit_check_from_env(),
        trace_snapshots: false,
    });
    // See the matching call in session.rs: clean up spawns an earlier,
    // killed run never got to start.
    hub.archive_unstarted_spawns();
    let notices = hub.take_notices();
    Ok(Headless {
        hub,
        background,
        completions,
        notices,
        model,
        effort: None,
        held_effort: None,
        mode,
        workspace_root,
        tool_filter,
        job_wait: job_wait_from_env(),
        trace: None,
        notes,
        judged: Vec::new(),
        criteria: None,
        criteria_due: false,
    })
}

/// The work mode and approval level `aura -p` agents run under for the
/// session mode `mode` (`--mode`, else `AURA_DEFAULT_MODE`) and the
/// `--approval` level, if given. Nobody can answer a prompt here, so every
/// ask is denied: without `--approval`, `agent` runs at `yolo` (commands
/// confined to the workspace) and the read-only modes at `ask`.
/// `AURA_TUI_MODE` and `AURA_TUI_APPROVAL` are the terminal's own and are
/// not read.
pub(crate) fn permissions(mode: AgentMode, approval: Option<ApprovalLevel>) -> Permissions {
    let mode = PermissionMode::from_agent_mode(mode);
    let approval = approval.unwrap_or(match mode {
        PermissionMode::Agent => ApprovalLevel::Yolo,
        PermissionMode::Plan | PermissionMode::ReadOnly | PermissionMode::Debug => {
            ApprovalLevel::Ask
        }
    });
    Permissions::new(mode, approval)
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
