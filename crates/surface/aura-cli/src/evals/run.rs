//! Running a suite's tasks and verifying their workspaces.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use aura_agent::AgentLoopConfig;
use aura_agent_trace::bundle::{now_ms, RunBundle};
use aura_agent_trace::{RunMeta, RunRecorder, RunSource, RunTrace};
use aura_app::headless::{EvalRan, EvalTurn};
use aura_app::recorded_runs::{self, HeadlessRun};
use aura_app::repo_source;
use aura_config::trace::TraceLevel;
use aura_core_types::trace::{Reward, RewardKind, RewardTarget};
use aura_exec_tools::ToolConfig;
use aura_model::model_catalog::GATE_MODEL_ID;
use serde_json::json;

use super::report::progress_line;
use super::{
    copy_dir, EvalGate, EvalOptions, EvalResult, EvalRunner, EvalTask, ProviderFactory, CHECKS_DIR,
    FIXTURE_DIR,
};

/// Run every task `opts.repeat` times, reporting each run on stderr as it
/// starts and ends so stdout stays the results table.
///
/// # Errors
///
/// Fails when a workspace can't be prepared or a run can't be recorded.
pub async fn run_suite(tasks: &[EvalTask], opts: &EvalOptions) -> anyhow::Result<Vec<EvalResult>> {
    let repeat = opts.repeat.max(1);
    let total = tasks.len() * repeat;
    let mut results = Vec::new();
    let mut spent = 0.0;
    for task in tasks {
        for _ in 0..repeat {
            if opts.budget_usd.is_some_and(|budget| spent >= budget) {
                tracing::warn!(spent, "Eval budget reached; skipping the remaining runs");
                eprintln!(
                    "budget reached (${spent:.4} spent); skipping the remaining {} of {total} runs",
                    total - results.len()
                );
                return Ok(results);
            }
            let n = results.len() + 1;
            eprintln!("[{n}/{total}] {} ...", task.id);
            let result = run_task(task, opts).await?;
            spent += result.cost_usd();
            eprintln!("{}", progress_line(n, total, &result, spent));
            results.push(result);
        }
    }
    Ok(results)
}

async fn run_task(task: &EvalTask, opts: &EvalOptions) -> anyhow::Result<EvalResult> {
    let task_id = format!("{}/{}", opts.suite, task.id);
    let meta = run_meta(task, opts, &task_id);
    let workspace = prepare_workspace(task, opts, &meta).await?;
    let mut prompts = vec![task.prompt.clone()];
    prompts.extend(task.follow_ups.iter().cloned());
    let run = TaskRun {
        task,
        opts,
        task_id,
        meta,
        workspace,
        prompts,
        checks: task.dir.join(CHECKS_DIR),
    };
    match &opts.runner {
        EvalRunner::Model {
            config,
            tool_config,
            provider,
        } => run.on_model(config, tool_config, provider).await,
        EvalRunner::Gate(gate) => run.on_gate(gate).await,
    }
}

/// The recorded run's metadata: its model, task, title and arm.
fn run_meta(task: &EvalTask, opts: &EvalOptions, task_id: &str) -> RunMeta {
    let model = match &opts.runner {
        EvalRunner::Model { config, .. } => config.model.clone(),
        EvalRunner::Gate(_) => GATE_MODEL_ID.to_string(),
    };
    let mut meta = RunMeta::new(model, RunSource::Eval);
    meta.task_id = Some(task_id.to_string());
    meta.title.clone_from(&task.title);
    meta.arm.clone_from(&opts.arm);
    meta
}

/// A fresh workspace for the run: the task's source repository, then its
/// fixture.
async fn prepare_workspace(
    task: &EvalTask,
    opts: &EvalOptions,
    meta: &RunMeta,
) -> anyhow::Result<PathBuf> {
    let workspace = opts.work_root.join(format!("{}-{}", task.id, meta.run_id));
    std::fs::create_dir_all(&workspace)?;
    if let Some(source) = &task.source {
        let repository = source.fetch(&repo_source::in_home(&opts.home)).await?;
        copy_dir(&repository, &workspace)?;
    }
    let fixture = task.dir.join(FIXTURE_DIR);
    if fixture.is_dir() {
        copy_dir(&fixture, &workspace)?;
    }
    Ok(workspace)
}

/// One run of a task with its workspace prepared.
struct TaskRun<'a> {
    task: &'a EvalTask,
    opts: &'a EvalOptions,
    task_id: String,
    meta: RunMeta,
    workspace: PathBuf,
    prompts: Vec<String>,
    checks: PathBuf,
}

impl TaskRun<'_> {
    /// The run under Gate: `aura -p` turns, then `verify` through the
    /// agent's kernel.
    async fn on_gate(self, gate: &EvalGate) -> anyhow::Result<EvalResult> {
        let Self {
            task,
            opts,
            task_id,
            mut meta,
            workspace,
            prompts,
            checks,
        } = self;
        let workspace = workspace.canonicalize()?;
        meta.workspace = workspace.display().to_string();
        let level = match opts.level {
            TraceLevel::Off => TraceLevel::Decisions,
            level => level,
        };
        let trace = RunRecorder::start(&opts.home, meta, level)?.context("tracing is off")?;
        let ran = gate
            .turns
            .lock()
            .await
            .run(EvalTurn {
                name: &format!("Eval {task_id}"),
                workspace: &workspace,
                prompts: &prompts,
                max_iterations: task.max_iterations,
                checks: checks.is_dir().then_some(checks.as_path()),
                verify: &task.verify,
                trace: Arc::clone(&trace),
            })
            .await?;
        let passed = ran.check.passed == Some(true);
        trace.add_rewards(&[gate_verify_reward(&task.verify, &ran, passed)]);
        let card = trace.finish().context("reading the run's trace")?;
        Ok(EvalResult {
            task: task.id.clone(),
            run_id: trace.run_id(),
            passed,
            error: ran.error,
            scorecard: card,
            gate: Some(ran.gate),
        })
    }

    /// The run on a model: the agent loop, then `verify` with `sh -c`.
    async fn on_model(
        self,
        config: &AgentLoopConfig,
        tool_config: &ToolConfig,
        provider: &ProviderFactory,
    ) -> anyhow::Result<EvalResult> {
        let Self {
            task,
            opts,
            meta,
            workspace,
            prompts,
            checks,
            ..
        } = self;
        let mut config = AgentLoopConfig::clone(config);
        if let Some(max) = task.max_iterations {
            config.max_iterations = max;
        }
        let outcome = recorded_runs::run(HeadlessRun {
            home: opts.home.clone(),
            workspace: workspace.clone(),
            provider: provider(task),
            config,
            prompts,
            meta,
            level: opts.level,
            tool_config: tool_config.clone(),
        })
        .await?;

        if checks.is_dir() {
            copy_dir(&checks, &workspace)?;
        }
        let (passed, reward) = verify(&task.verify, &workspace, "eval")?;
        let bundle = RunBundle::at(&outcome.bundle_dir);
        bundle.append_rewards(&[reward])?;
        let card = RunTrace::load(&bundle)?.scorecard();
        bundle.write_summary(&card)?;
        Ok(EvalResult {
            task: task.id.clone(),
            run_id: outcome.run_id,
            passed,
            error: outcome.error,
            scorecard: card,
            gate: None,
        })
    }
}

/// The outcome reward of a Gate run's `verify`, run through `run_command`.
fn gate_verify_reward(cmd: &str, ran: &EvalRan, passed: bool) -> Reward {
    Reward {
        target: RewardTarget::Run,
        kind: RewardKind::Outcome,
        name: "verify".into(),
        value: if passed { 1.0 } else { 0.0 },
        source: "eval".into(),
        at_ms: now_ms(),
        evidence: json!({
            "cmd": cmd,
            "exit_code": ran.check.exit_code,
            "ms": ran.check_ms,
            "output_tail": ran.check.tail,
            "via": "run_command",
        }),
    }
}

/// Run `cmd` with `sh -c` in `dir` and turn its exit status into an
/// outcome reward (1 passed, 0 failed) with the output tails as evidence.
/// Cargo builds into `dir/target`, so workspaces of crates with the same
/// name never share build results.
///
/// # Errors
///
/// Fails when the shell can't be started.
pub fn verify(cmd: &str, dir: &Path, source: &str) -> anyhow::Result<(bool, Reward)> {
    let started = now_ms();
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", dir.join("target"))
        .output()
        .with_context(|| format!("running `{cmd}` in {}", dir.display()))?;
    let passed = output.status.success();
    let tail = |b: &[u8]| {
        let s = String::from_utf8_lossy(b);
        let lines: Vec<&str> = s.lines().rev().take(20).collect();
        lines.into_iter().rev().collect::<Vec<_>>().join("\n")
    };
    let reward = Reward {
        target: RewardTarget::Run,
        kind: RewardKind::Outcome,
        name: "verify".into(),
        value: if passed { 1.0 } else { 0.0 },
        source: source.into(),
        at_ms: now_ms(),
        evidence: json!({
            "cmd": cmd,
            "exit_code": output.status.code(),
            "ms": now_ms().saturating_sub(started),
            "stdout_tail": tail(&output.stdout),
            "stderr_tail": tail(&output.stderr),
        }),
    };
    Ok((passed, reward))
}
