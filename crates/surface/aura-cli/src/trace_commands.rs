//! `aura runs` and `aura dataset`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context};
use aura_agent::AgentLoopConfig;
use aura_agent_trace::bundle::{list_runs, now_ms, RunBundle};
use aura_agent_trace::dataset::{self, ExportFilter, ExportFormat};
use aura_agent_trace::{health, report, scorecard, RunScorecard, RunTrace};
use aura_config::HarnessParams;
use aura_core_types::trace::{Reward, RewardKind, RewardTarget};
use aura_exec_tools::ToolConfig;
use aura_model::model_catalog::GATE_MODEL_ID;
use aura_model::ModelProvider;
use serde_json::json;

use crate::cli::{
    DatasetCommand, DatasetSubcommand, EvalCommand, EvalSubcommand, HarnessArgs, RunsCommand,
    RunsSubcommand,
};
use crate::evals::{self, EvalGate, EvalOptions, EvalRunner, EvalTask};
use aura_app::model_selection::{may_be_alias, ModelSelection};
use aura_app::recorded_runs::{self, HeadlessRun};
use aura_app::{event_loop, session_helpers};

fn home() -> anyhow::Result<std::path::PathBuf> {
    Ok(aura_config::AuraHome::resolve()
        .context("resolving AURA_HOME")?
        .path)
}

fn load(home: &Path, id: &str) -> anyhow::Result<(RunBundle, RunTrace)> {
    let bundle = RunBundle::find(home, id)?;
    let trace = RunTrace::load(&bundle)?;
    Ok((bundle, trace))
}

/// `aura runs ...`
///
/// # Errors
///
/// Fails when a run can't be found or read.
pub async fn cmd_runs(args: RunsCommand) -> anyhow::Result<()> {
    let home = home()?;
    match args.action {
        RunsSubcommand::List { limit, json } => runs_list(&home, limit, json),
        RunsSubcommand::Show { id, steps, json } => runs_show(&home, &id, steps, json),
        RunsSubcommand::Check {
            id,
            slow_secs,
            json,
        } => runs_check(&home, &id, slow_secs, json),
        RunsSubcommand::Diff {
            baseline,
            candidate,
            tolerance,
        } => runs_diff(&home, &baseline, &candidate, tolerance),
        RunsSubcommand::Label {
            id,
            value,
            step,
            name,
            note,
        } => runs_label(&home, &id, value, step, name, note),
        RunsSubcommand::Verify { id, cmd } => runs_verify(&home, &id, &cmd),
        RunsSubcommand::Branch {
            id,
            from_step,
            workspace,
            harness,
        } => runs_branch(&home, &id, from_step, &workspace, &harness).await,
    }
}

/// `aura runs list`
fn runs_list(home: &Path, limit: usize, json: bool) -> anyhow::Result<()> {
    let mut runs = list_runs(home);
    runs.truncate(limit);
    if json {
        println!("{}", serde_json::to_string_pretty(&runs)?);
    } else if runs.is_empty() {
        println!("(no runs under {})", home.join("runs").display());
    } else {
        print!("{}", report::runs_table(&runs));
    }
    Ok(())
}

/// `aura runs show`
fn runs_show(home: &Path, id: &str, steps: bool, json: bool) -> anyhow::Result<()> {
    let (bundle, trace) = load(home, id)?;
    let health = health::check(&trace, now_ms(), health::DEFAULT_SLOW_MS);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "meta": trace.meta,
                "scorecard": trace.scorecard(),
                "health": health,
                "steps": trace.steps,
                "rewards": trace.rewards(),
            }))?
        );
        return Ok(());
    }
    println!("{}", bundle.dir().display());
    if !trace.meta.title.is_empty() {
        println!("title     {}", trace.meta.title);
    }
    print!("{}", report::scorecard_text(&trace.scorecard()));
    print!("{}", report::health_text(&health));
    if steps {
        println!();
        for step in &trace.steps {
            println!("{}", report::step_line(step));
        }
    }
    Ok(())
}

/// `aura runs check`
fn runs_check(home: &Path, id: &str, slow_secs: u64, json: bool) -> anyhow::Result<()> {
    let (_, trace) = load(home, id)?;
    let health = health::check(&trace, now_ms(), slow_secs.saturating_mul(1_000));
    if json {
        println!("{}", serde_json::to_string_pretty(&health)?);
    } else {
        print!("{}", report::health_text(&health));
    }
    if !health.is_healthy() {
        bail!(
            "run {} has {} problem(s)",
            trace.meta.run_id,
            health.problems.len()
        );
    }
    Ok(())
}

/// `aura runs diff`
fn runs_diff(home: &Path, baseline: &str, candidate: &str, tolerance: f64) -> anyhow::Result<()> {
    let (_, a) = load(home, baseline)?;
    let (_, b) = load(home, candidate)?;
    let deltas = scorecard::diff(&a.scorecard(), &b.scorecard(), tolerance);
    print!("{}", report::diff_table(&deltas));
    let regressions = deltas.iter().filter(|d| d.regression).count();
    if regressions > 0 {
        println!(
            "{regressions} regression(s) beyond {:.0}%",
            tolerance * 100.0
        );
    }
    Ok(())
}

/// `aura runs label`
fn runs_label(
    home: &Path,
    id: &str,
    value: f64,
    step: Option<u64>,
    name: String,
    note: Option<String>,
) -> anyhow::Result<()> {
    let (bundle, trace) = load(home, id)?;
    if let Some(seq) = step {
        if trace.step(seq).is_none() {
            bail!("run {} has no step {seq}", trace.meta.run_id);
        }
    }
    bundle.append_rewards(&[Reward {
        target: step.map_or(RewardTarget::Run, |seq| RewardTarget::Step { seq }),
        kind: RewardKind::HumanExplicit,
        name,
        value,
        source: "cli".into(),
        at_ms: now_ms(),
        evidence: note.map_or(serde_json::Value::Null, |n| json!({ "note": n })),
    }])?;
    println!("labeled {}", trace.meta.run_id);
    Ok(())
}

/// `aura runs verify`
fn runs_verify(home: &Path, id: &str, cmd: &str) -> anyhow::Result<()> {
    let (bundle, trace) = load(home, id)?;
    let dir = if trace.meta.workspace.is_empty() {
        std::env::current_dir()?
    } else {
        trace.meta.workspace.clone().into()
    };
    let (passed, reward) = evals::verify(cmd, &dir, "cli")?;
    bundle.append_rewards(&[reward])?;
    println!(
        "{} {}: `{cmd}` {}",
        if passed { "✓" } else { "✗" },
        trace.meta.run_id,
        if passed { "passed" } else { "failed" }
    );
    Ok(())
}

/// `aura runs branch`
async fn runs_branch(
    home: &Path,
    id: &str,
    from_step: u64,
    workspace: &Path,
    harness: &HarnessArgs,
) -> anyhow::Result<()> {
    let (_, source) = load(home, id)?;
    let arm = install_params(harness)?;
    let model = harness
        .model
        .clone()
        .unwrap_or_else(|| source.meta.model.clone());
    let (config, live) = loop_setup(&harness.provider, Some(&model)).await?;
    let plan = recorded_runs::plan_branch(&source, from_step)?;
    let mut meta = plan.meta;
    meta.arm = arm;
    let provider = recorded_runs::replay_provider(home, &source, from_step, live)?;
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("workspace {}", workspace.display()))?;
    let outcome = recorded_runs::run(HeadlessRun {
        home: home.to_path_buf(),
        workspace,
        provider,
        config,
        prompts: plan.prompts,
        meta,
        level: aura_config::trace().level,
        tool_config: ToolConfig::for_autonomous_dev_loop(),
    })
    .await?;
    println!(
        "branch {} of {} from step {from_step}",
        outcome.run_id, source.meta.run_id
    );
    if let Some(card) = &outcome.scorecard {
        print!("{}", report::scorecard_text(card));
    }
    if let Some(e) = outcome.error {
        println!("ended early: {e}");
    }
    Ok(())
}

/// `aura dataset ...`
///
/// # Errors
///
/// Fails on an unknown format, a bad filter, or an I/O error.
pub fn cmd_dataset(args: DatasetCommand) -> anyhow::Result<()> {
    let home = home()?;
    match args.action {
        DatasetSubcommand::Build {
            format,
            filter,
            out,
        } => {
            let Some(format) = ExportFormat::parse(&format) else {
                bail!("unknown format `{format}` (sft, preference, rl, harness-bandit)");
            };
            let filter = ExportFilter::parse(&filter)?;
            let out = out.unwrap_or_else(|| dataset::default_out_dir(&home, format));
            let traces = dataset::load_all(&home);
            let card = dataset::export(&traces, Some(&home), format, &filter, &out)?;
            println!(
                "{}: {} of {} runs → {} ({} duplicates dropped, {} redactions)",
                card.format,
                card.runs_used,
                card.runs_seen,
                out.display(),
                card.duplicates,
                card.redactions
            );
            for (split, n) in &card.records {
                println!("  {split}.jsonl  {n} records");
            }
        }
    }
    Ok(())
}

/// `aura eval ...`
///
/// # Errors
///
/// Fails on an invalid suite, a provider that can't be built, or (with
/// `--fail-on-regression`) a regressing arm.
pub async fn cmd_eval(args: EvalCommand) -> anyhow::Result<()> {
    let home = home()?;
    match args.action {
        EvalSubcommand::Run {
            suite,
            task,
            repeat,
            work_dir,
            budget_usd,
            harness,
        } => {
            let tasks = selected_tasks(&suite, &task)?;
            let arm = install_params(&harness)?;
            let runner = eval_runner(&harness).await?;
            let opts = EvalOptions {
                home: home.clone(),
                suite: evals::suite_name(&suite),
                arm,
                repeat,
                level: aura_config::trace().level,
                runner,
                work_root: work_dir.unwrap_or_else(|| home.join("evals").join("work")),
                budget_usd,
            };
            let results = evals::run_suite(&tasks, &opts).await?;
            print!("{}", evals::results_table(&results));
            Ok(())
        }
        EvalSubcommand::Compare {
            suite,
            arms,
            tolerance,
            fail_on_regression,
            baseline,
            write_baseline,
        } => eval_compare(
            &home,
            suite.as_deref(),
            &arms,
            tolerance,
            fail_on_regression,
            baseline.as_deref(),
            write_baseline.as_deref(),
        ),
    }
}

/// `aura eval compare`: print the arm comparison, or write a baseline.
fn eval_compare(
    home: &Path,
    suite: Option<&str>,
    arms: &[String],
    tolerance: f64,
    fail_on_regression: bool,
    baseline: Option<&Path>,
    write_baseline: Option<&Path>,
) -> anyhow::Result<()> {
    let runs = evals::runs_by_arm(home, suite);
    if let Some(path) = write_baseline {
        return write_baseline_file(&runs, arms, suite, path);
    }
    let (report, regressions) = match baseline {
        Some(path) => evals::Baseline::load(path)?.compare(&runs, arms, tolerance)?,
        None => evals::compare(&runs, arms, tolerance)?,
    };
    print!("{report}");
    if fail_on_regression && regressions > 0 {
        bail!(
            "{regressions} regression(s) beyond {:.0}%",
            tolerance * 100.0
        );
    }
    Ok(())
}

/// The suite's tasks, only those named in `task` when it isn't empty.
fn selected_tasks(suite: &Path, task: &[String]) -> anyhow::Result<Vec<EvalTask>> {
    let mut tasks = evals::load_suite(suite)?;
    if !task.is_empty() {
        tasks.retain(|t| task.contains(&t.id));
        if tasks.is_empty() {
            bail!("no task in {} matches {task:?}", suite.display());
        }
    }
    Ok(tasks)
}

/// Gate for `--model gate`, else the agent loop on the chosen model.
async fn eval_runner(harness: &HarnessArgs) -> anyhow::Result<EvalRunner> {
    if harness.model.as_deref() == Some(GATE_MODEL_ID) {
        return Ok(EvalRunner::Gate(EvalGate::start(&harness.provider).await?));
    }
    let (config, provider) = loop_setup(&harness.provider, harness.model.as_deref()).await?;
    Ok(EvalRunner::Model {
        config: Box::new(config),
        tool_config: ToolConfig::for_autonomous_dev_loop(),
        provider: Box::new(move |_| Arc::clone(&provider)),
    })
}

/// `aura eval compare --write-baseline`: the first arm's numbers.
fn write_baseline_file(
    runs: &BTreeMap<String, Vec<(String, RunScorecard)>>,
    arms: &[String],
    suite: Option<&str>,
    path: &Path,
) -> anyhow::Result<()> {
    let arm = arms
        .first()
        .cloned()
        .unwrap_or_else(|| evals::DEFAULT_ARM.to_string());
    let baseline = evals::Baseline::from_runs(runs, &arm, suite)?;
    baseline.save(path)?;
    println!(
        "wrote baseline for arm {arm} ({} runs) to {}",
        baseline.runs,
        path.display()
    );
    Ok(())
}

/// Apply `--set key=value` overrides to the process-wide harness
/// parameters and return the arm label for the run.
fn install_params(args: &HarnessArgs) -> anyhow::Result<Option<String>> {
    if args.set.is_empty() {
        return Ok(args.arm.clone());
    }
    let current = HarnessParams::current();
    let known = serde_json::to_value(current)?;
    let mut overrides = serde_json::Map::new();
    for pair in &args.set {
        let Some((key, value)) = pair.split_once('=') else {
            bail!("`--set {pair}` is not key=value");
        };
        let key = key.trim();
        if known.get(key).is_none() {
            let names: Vec<&String> = known
                .as_object()
                .map(|o| o.keys().collect())
                .unwrap_or_default();
            bail!("unknown harness parameter `{key}` (known: {names:?})");
        }
        let value = serde_json::from_str(value.trim())
            .unwrap_or_else(|_| serde_json::Value::String(value.trim().to_string()));
        overrides.insert(key.to_string(), value);
    }
    current
        .with_overrides(&serde_json::Value::Object(overrides))
        .context("applying --set overrides")?
        .install();
    Ok(Some(args.arm.clone().unwrap_or_else(|| args.set.join(","))))
}

/// Loop configuration and provider for a headless run. The model is
/// `model` or the terminal's startup model; the choice is not persisted.
/// `gate` runs every run on Gate's default pick for the current sign-ins
/// (`aura runs branch`; `aura eval run` routes each task with Gate).
async fn loop_setup(
    provider: &str,
    model: Option<&str>,
) -> anyhow::Result<(AgentLoopConfig, Arc<dyn ModelProvider + Send + Sync>)> {
    let mut config = session_helpers::default_agent_config(aura_model::ENV_FALLBACK_MODEL);
    if provider == "mock" {
        if let Some(model) = model {
            config.model = model.to_string();
        }
        return Ok((config, aura_model::mock_provider().provider));
    }
    let auth = Arc::new(aura_auth::AuthManager::new().context("loading sign-in state")?);
    let summary = event_loop::auth::auth_summary(&auth, config.auth_token.as_deref());
    let mut models = ModelSelection::init(&mut config, summary);
    // A short name (`opus`, `deepseek`) resolves among the listed models,
    // so load the live lists before resolving it.
    let alias = model.is_some_and(may_be_alias);
    if alias {
        models.refresh_local().await;
        if let Some(note) = models.refresh_plans(auth.as_ref()).await {
            eprintln!("{note}");
        }
    }
    if let Some(model) = model {
        models
            .use_for_run(&mut config, model)
            .map_err(|problem| anyhow::anyhow!("--model {model}: {problem}"))?;
    }
    if models.is_gate() {
        if !alias {
            models.refresh_local().await;
            if let Some(note) = models.refresh_plans(auth.as_ref()).await {
                eprintln!("{note}");
            }
        }
        models.refresh_route(&mut config);
        eprintln!("{}", models.display());
    }
    let resolver: Arc<dyn aura_auth::CredentialResolver> = auth;
    let selection =
        aura_model::routed_provider(Some(resolver)).context("building model provider")?;
    Ok((config, selection.provider))
}
