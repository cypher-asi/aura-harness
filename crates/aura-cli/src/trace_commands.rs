//! `aura runs` and `aura dataset`.

use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context};
use aura_agent::AgentLoopConfig;
use aura_agent_trace::bundle::{list_runs, now_ms, RunBundle};
use aura_agent_trace::dataset::{self, ExportFilter, ExportFormat};
use aura_agent_trace::{health, report, scorecard, RunTrace};
use aura_config::HarnessParams;
use aura_core_types::trace::{Reward, RewardKind, RewardTarget};
use aura_model_reasoner::ModelProvider;
use aura_tools::ToolConfig;
use serde_json::json;

use crate::cli::{
    DatasetCommand, DatasetSubcommand, EvalCommand, EvalSubcommand, HarnessArgs, RunsCommand,
    RunsSubcommand,
};
use crate::evals::{self, EvalOptions};
use aura_app::model_selection::ModelSelection;
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
        RunsSubcommand::List { limit, json } => {
            let mut runs = list_runs(&home);
            runs.truncate(limit);
            if json {
                println!("{}", serde_json::to_string_pretty(&runs)?);
            } else if runs.is_empty() {
                println!("(no runs under {})", home.join("runs").display());
            } else {
                print!("{}", report::runs_table(&runs));
            }
        }
        RunsSubcommand::Show { id, steps, json } => {
            let (bundle, trace) = load(&home, &id)?;
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
        }
        RunsSubcommand::Check {
            id,
            slow_secs,
            json,
        } => {
            let (_, trace) = load(&home, &id)?;
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
        }
        RunsSubcommand::Diff {
            baseline,
            candidate,
            tolerance,
        } => {
            let (_, a) = load(&home, &baseline)?;
            let (_, b) = load(&home, &candidate)?;
            let deltas = scorecard::diff(&a.scorecard(), &b.scorecard(), tolerance);
            print!("{}", report::diff_table(&deltas));
            let regressions = deltas.iter().filter(|d| d.regression).count();
            if regressions > 0 {
                println!(
                    "{regressions} regression(s) beyond {:.0}%",
                    tolerance * 100.0
                );
            }
        }
        RunsSubcommand::Label {
            id,
            value,
            step,
            name,
            note,
        } => {
            let (bundle, trace) = load(&home, &id)?;
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
        }
        RunsSubcommand::Verify { id, cmd } => {
            let (bundle, trace) = load(&home, &id)?;
            let dir = if trace.meta.workspace.is_empty() {
                std::env::current_dir()?
            } else {
                trace.meta.workspace.clone().into()
            };
            let (passed, reward) = evals::verify(&cmd, &dir, "cli")?;
            bundle.append_rewards(&[reward])?;
            println!(
                "{} {}: `{cmd}` {}",
                if passed { "✓" } else { "✗" },
                trace.meta.run_id,
                if passed { "passed" } else { "failed" }
            );
        }
        RunsSubcommand::Branch {
            id,
            from_step,
            workspace,
            harness,
        } => {
            let (_, source) = load(&home, &id)?;
            let arm = install_params(&harness)?;
            let model = harness
                .model
                .clone()
                .unwrap_or_else(|| source.meta.model.clone());
            let (config, live) = loop_setup(&harness.provider, Some(&model))?;
            let plan = recorded_runs::plan_branch(&source, from_step)?;
            let mut meta = plan.meta;
            meta.arm = arm;
            let provider = recorded_runs::replay_provider(&home, &source, from_step, live)?;
            let workspace = workspace
                .canonicalize()
                .with_context(|| format!("workspace {}", workspace.display()))?;
            let outcome = recorded_runs::run(HeadlessRun {
                home: home.clone(),
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
        }
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
            harness,
        } => {
            let mut tasks = evals::load_suite(&suite)?;
            if !task.is_empty() {
                tasks.retain(|t| task.contains(&t.id));
                if tasks.is_empty() {
                    bail!("no task in {} matches {task:?}", suite.display());
                }
            }
            let arm = install_params(&harness)?;
            let (config, provider) = loop_setup(&harness.provider, harness.model.as_deref())?;
            let opts = EvalOptions {
                home: home.clone(),
                suite: evals::suite_name(&suite),
                arm,
                repeat,
                level: aura_config::trace().level,
                config,
                tool_config: ToolConfig::for_autonomous_dev_loop(),
                work_root: work_dir.unwrap_or_else(|| home.join("evals").join("work")),
                provider: Box::new(move |_| Arc::clone(&provider)),
            };
            let results = evals::run_suite(&tasks, &opts).await?;
            print!("{}", evals::results_table(&results));
        }
        EvalSubcommand::Compare {
            suite,
            arms,
            tolerance,
            fail_on_regression,
            baseline,
            write_baseline,
        } => {
            let runs = evals::runs_by_arm(&home, suite.as_deref());
            if let Some(path) = write_baseline {
                let arm = arms
                    .first()
                    .cloned()
                    .unwrap_or_else(|| evals::DEFAULT_ARM.to_string());
                let baseline = evals::Baseline::from_runs(&runs, &arm, suite.as_deref())?;
                baseline.save(&path)?;
                println!(
                    "wrote baseline for arm {arm} ({} runs) to {}",
                    baseline.runs,
                    path.display()
                );
                return Ok(());
            }
            let (report, regressions) = match baseline {
                Some(path) => evals::Baseline::load(&path)?.compare(&runs, &arms, tolerance)?,
                None => evals::compare(&runs, &arms, tolerance)?,
            };
            print!("{report}");
            if fail_on_regression && regressions > 0 {
                bail!(
                    "{regressions} regression(s) beyond {:.0}%",
                    tolerance * 100.0
                );
            }
        }
    }
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
fn loop_setup(
    provider: &str,
    model: Option<&str>,
) -> anyhow::Result<(AgentLoopConfig, Arc<dyn ModelProvider + Send + Sync>)> {
    let mut config = session_helpers::default_agent_config(aura_model_reasoner::ENV_FALLBACK_MODEL);
    if provider == "mock" {
        if let Some(model) = model {
            config.model = model.to_string();
        }
        return Ok((config, aura_model_reasoner::mock_provider().provider));
    }
    let auth = Arc::new(aura_auth::AuthManager::new().context("loading sign-in state")?);
    let summary = event_loop::auth::auth_summary(&auth, config.auth_token.as_deref());
    let models = ModelSelection::init(&mut config, summary);
    if let Some(model) = model {
        if !models.usable(model) {
            bail!("model `{model}` is not usable with the current sign-in");
        }
        models.limits().configure(&mut config, model);
    }
    let resolver: Arc<dyn aura_auth::CredentialResolver> = auth;
    let selection =
        aura_model_reasoner::routed_provider(Some(resolver)).context("building model provider")?;
    Ok((config, selection.provider))
}
