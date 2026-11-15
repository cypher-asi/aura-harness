//! `aura runs` and `aura dataset`.

use std::path::Path;

use anyhow::{bail, Context};
use aura_agent_trace::bundle::{list_runs, now_ms, RunBundle};
use aura_agent_trace::dataset::{self, ExportFilter, ExportFormat};
use aura_agent_trace::{report, scorecard, RunTrace};
use aura_core_types::trace::{Reward, RewardKind, RewardTarget};
use serde_json::json;

use crate::cli::{DatasetCommand, DatasetSubcommand, RunsCommand, RunsSubcommand};

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
pub fn cmd_runs(args: RunsCommand) -> anyhow::Result<()> {
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
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "meta": trace.meta,
                        "scorecard": trace.scorecard(),
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
            if steps {
                println!();
                for step in &trace.steps {
                    println!("{}", report::step_line(step));
                }
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
            let started = now_ms();
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .current_dir(&dir)
                .output()
                .with_context(|| format!("running `{cmd}` in {}", dir.display()))?;
            let passed = output.status.success();
            let tail = |b: &[u8]| {
                let s = String::from_utf8_lossy(b);
                let lines: Vec<&str> = s.lines().rev().take(20).collect();
                lines.into_iter().rev().collect::<Vec<_>>().join("\n")
            };
            bundle.append_rewards(&[Reward {
                target: RewardTarget::Run,
                kind: RewardKind::Outcome,
                name: "verify".into(),
                value: if passed { 1.0 } else { 0.0 },
                source: "cli".into(),
                at_ms: now_ms(),
                evidence: json!({
                    "cmd": cmd,
                    "exit_code": output.status.code(),
                    "ms": now_ms().saturating_sub(started),
                    "stdout_tail": tail(&output.stdout),
                    "stderr_tail": tail(&output.stderr),
                }),
            }])?;
            println!(
                "{} {}: `{cmd}` {}",
                if passed { "✓" } else { "✗" },
                trace.meta.run_id,
                if passed { "passed" } else { "failed" }
            );
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
