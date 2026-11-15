//! `aura gate`: report, refit, benchmark and promote the gate's routing
//! policy.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};
use aura_agent_gate::{fit, GatePolicy, Ledger, LedgerEntry};
use aura_agent_trace::bundle::list_runs;
use aura_agent_trace::RunSource;
use aura_config::gate::{GateConfig, GATE_DIR, LEDGER_FILE, POLICY_FILE};

use crate::cli::{GateCommand, GateSubcommand};
use crate::{evals, gate_bench};

/// Fitted policies, by version, under the gate directory.
const POLICIES_DIR: &str = "policies";

/// `aura gate ...`
///
/// # Errors
/// Fails when the ledger, a policy or the suite can't be read, or when
/// `promote` finds the benchmark does not support the policy.
pub fn cmd_gate(args: GateCommand) -> anyhow::Result<()> {
    let home = aura_config::AuraHome::resolve()
        .context("resolving AURA_HOME")?
        .path;
    let config = GateConfig::from_env(Some(&home));
    let gate_dir = home.join(GATE_DIR);
    let ledger_path = config
        .ledger
        .clone()
        .unwrap_or_else(|| gate_dir.join(LEDGER_FILE));
    let entries = if ledger_path.is_file() {
        Ledger::read(&ledger_path).with_context(|| format!("reading {}", ledger_path.display()))?
    } else {
        Vec::new()
    };
    let active = crate::gate::settings(Some(&home), None).policy;

    match args.action {
        GateSubcommand::Report => {
            match &config.policy {
                Some(path) => println!("policy file {}", path.display()),
                None => println!("policy file (none: built-in rules)"),
            }
            println!(
                "ledger      {} ({} attempts)",
                ledger_path.display(),
                entries.len()
            );
            print!("{}", fit::report(&active, &entries));
        }
        GateSubcommand::Fit { version } => {
            let dir = gate_dir.join(POLICIES_DIR);
            let version = version.unwrap_or_else(|| next_version(&dir));
            let policy = fit::fit(&active, &entries, &run_outcomes(&home), &version);
            let path = dir.join(format!("{version}.json"));
            policy.save(&path).map_err(|e| anyhow!(e))?;
            print!("{}", fit::report(&policy, &entries));
            println!("wrote {}", path.display());
        }
        GateSubcommand::Bench {
            suite,
            reference,
            arms,
            cheap,
            margin,
        } => {
            let (text, _) = bench(&home, &suite, &reference, &arms, cheap.as_deref(), margin)?;
            print!("{text}");
        }
        GateSubcommand::Promote {
            policy,
            suite,
            arm,
            reference,
            cheap,
            margin,
            force,
        } => {
            let candidate = GatePolicy::load(&policy).map_err(|e| anyhow!(e))?;
            if !force {
                check_arm_policy(&home, &suite, &arm, &candidate, &entries)?;
                let (text, verdicts) = bench(
                    &home,
                    &suite,
                    &reference,
                    std::slice::from_ref(&arm),
                    cheap.as_deref(),
                    margin,
                )?;
                print!("{text}");
                let verdict = &verdicts[0];
                if !verdict.wins() {
                    bail!(
                        "not promoted: {arm} costs {:.2}× {reference} with Δpass {:+.3} \
                         (95% low {:+.3}, margin {margin:.2}){}",
                        verdict.cost_ratio,
                        verdict.delta,
                        verdict.delta_low,
                        if verdict.beats_mixture == Some(false) {
                            ", and does not beat the cost-matched mixture"
                        } else {
                            ""
                        }
                    );
                }
                if !verdict.weak_categories.is_empty() {
                    bail!(
                        "not promoted: not shown non-inferior in {}",
                        verdict.weak_categories.join(", ")
                    );
                }
            }
            let target = gate_dir.join(POLICY_FILE);
            candidate.save(&target).map_err(|e| anyhow!(e))?;
            println!("promoted {} to {}", candidate.version, target.display());
            if config.policy.as_deref().is_some_and(|p| p != target) {
                println!("note: AURA_GATE_POLICY points elsewhere, so it still wins");
            }
        }
    }
    Ok(())
}

fn bench(
    home: &Path,
    suite: &Path,
    reference: &str,
    arms: &[String],
    cheap: Option<&str>,
    margin: f64,
) -> anyhow::Result<(String, Vec<gate_bench::Verdict>)> {
    let name = evals::suite_name(suite);
    let categories = evals::categories(&name, &evals::load_suite(suite)?);
    let runs = evals::runs_by_arm(home, Some(&name));
    gate_bench::report(&runs, &categories, reference, arms, cheap, margin)
}

/// Fail unless every gate attempt of `arm`'s runs was routed by
/// `policy`, so the benchmark measures that policy.
fn check_arm_policy(
    home: &Path,
    suite: &Path,
    arm: &str,
    policy: &GatePolicy,
    entries: &[LedgerEntry],
) -> anyhow::Result<()> {
    let prefix = format!("{}/", evals::suite_name(suite));
    let run_ids: BTreeSet<String> = list_runs(home)
        .into_iter()
        .filter(|(meta, _)| meta.source == RunSource::Eval)
        .filter(|(meta, _)| meta.arm.as_deref() == Some(arm))
        .filter(|(meta, _)| {
            meta.task_id
                .as_deref()
                .is_some_and(|t| t.starts_with(&prefix))
        })
        .map(|(meta, _)| meta.run_id)
        .collect();
    let used: BTreeSet<&str> = entries
        .iter()
        .filter(|e| e.run_id.as_ref().is_some_and(|id| run_ids.contains(id)))
        .map(|e| e.policy.as_str())
        .collect();
    if used.is_empty() {
        bail!("arm {arm} has no gate attempts in the ledger; run the suite with a gate model");
    }
    if used.len() > 1 || !used.contains(policy.version.as_str()) {
        bail!(
            "arm {arm} was routed by {used:?}, not {}; rerun it with AURA_GATE_POLICY set to the policy",
            policy.version
        );
    }
    Ok(())
}

/// Whether each recorded run with an outcome passed.
fn run_outcomes(home: &Path) -> HashMap<String, bool> {
    list_runs(home)
        .into_iter()
        .filter_map(|(meta, card)| Some((meta.run_id, card?.outcome? > 0.0)))
        .collect()
}

/// `v<n+1>` for the highest `v<n>.json` in `dir`.
fn next_version(dir: &Path) -> String {
    let highest = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = PathBuf::from(e.file_name());
            let stem = name.file_stem()?.to_str()?.to_string();
            stem.strip_prefix('v')?.parse::<u32>().ok()
        })
        .max()
        .unwrap_or(0);
    format!("v{}", highest + 1)
}

#[cfg(test)]
mod tests {
    use super::next_version;

    #[test]
    fn versions_count_up_from_the_highest() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(next_version(dir.path()), "v1");
        std::fs::write(dir.path().join("v2.json"), "{}").unwrap();
        std::fs::write(dir.path().join("v10.json"), "{}").unwrap();
        std::fs::write(dir.path().join("draft.json"), "{}").unwrap();
        assert_eq!(next_version(dir.path()), "v11");
    }
}
