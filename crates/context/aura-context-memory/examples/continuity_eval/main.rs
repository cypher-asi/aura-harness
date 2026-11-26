//! Deterministic Agent Continuity benchmark and cross-agent report adapter.

// Example code: it may unwrap/expect.
#![allow(clippy::unwrap_used, clippy::expect_used)]
// Scenario and selection counts are deliberately tiny and exactly representable
// as f64; the casts keep the emitted report schema straightforward.
#![allow(clippy::cast_precision_loss)]

mod aura_run;
mod bench_store;
mod scope_isolation;
mod scoring;
mod suite;

use std::path::{Path, PathBuf};

use aura_context_memory::MemoryRetrievalMode;
use chrono::Utc;
use serde::Serialize;

use crate::aura_run::run_aura;
use crate::scope_isolation::{evaluate_scope_isolation, ScopeIsolationReport};
use crate::scoring::{score_external, AgentReport};
use crate::suite::{EvalSuite, ExternalComparison, DEFAULT_SUITE};

#[derive(Debug, Serialize)]
struct Thresholds {
    minimum_recall_at_k: f64,
    minimum_precision_at_k: f64,
    maximum_forbidden_recall_rate: f64,
    maximum_average_estimated_tokens: f64,
}

impl Thresholds {
    fn passed_by(&self, aura: &AgentReport, scope_isolation: &ScopeIsolationReport) -> bool {
        aura.recall_at_k >= self.minimum_recall_at_k
            && aura.precision_at_k >= self.minimum_precision_at_k
            && aura.forbidden_recall_rate <= self.maximum_forbidden_recall_rate
            && aura.average_estimated_tokens <= self.maximum_average_estimated_tokens
            && scope_isolation.checks_passed == scope_isolation.checks_total
            && scope_isolation.forbidden_leakage_rate == 0.0
    }
}

#[derive(Debug, Serialize)]
struct SuiteReport {
    suite_version: u32,
    generated_at: String,
    thresholds: Thresholds,
    aura_passed: bool,
    aura_improvement_vs_salience: AuraImprovement,
    scope_isolation: ScopeIsolationReport,
    reports: Vec<AgentReport>,
}

#[derive(Debug, Serialize)]
struct AuraImprovement {
    recall_at_k_delta: f64,
    precision_at_k_delta: f64,
    average_estimated_tokens_reduction: f64,
    estimated_token_reduction_percent: f64,
}

impl AuraImprovement {
    fn between(aura: &AgentReport, salience_baseline: &AgentReport) -> Self {
        Self {
            recall_at_k_delta: aura.recall_at_k - salience_baseline.recall_at_k,
            precision_at_k_delta: aura.precision_at_k - salience_baseline.precision_at_k,
            average_estimated_tokens_reduction: salience_baseline.average_estimated_tokens
                - aura.average_estimated_tokens,
            estimated_token_reduction_percent: if salience_baseline.average_estimated_tokens > 0.0 {
                (salience_baseline.average_estimated_tokens - aura.average_estimated_tokens)
                    / salience_baseline.average_estimated_tokens
                    * 100.0
            } else {
                0.0
            },
        }
    }
}

fn arguments() -> (Option<PathBuf>, Vec<PathBuf>) {
    let mut output = None;
    let mut comparisons = Vec::new();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--output" => {
                index += 1;
                output = args.get(index).map(PathBuf::from);
            }
            "--comparison" => {
                index += 1;
                if let Some(path) = args.get(index) {
                    comparisons.push(PathBuf::from(path));
                }
            }
            unknown => panic!("unknown argument: {unknown}"),
        }
        index += 1;
    }
    (output, comparisons)
}

fn load_comparison(path: &Path) -> ExternalComparison {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read comparison {}: {error}", path.display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|error| panic!("parse comparison {}: {error}", path.display()))
}

fn write_report(path: &Path, json: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create report directory");
    }
    std::fs::write(path, format!("{json}\n")).expect("write benchmark report");
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let (output, comparison_paths) = arguments();
    let suite: EvalSuite = serde_json::from_str(DEFAULT_SUITE).expect("parse benchmark suite");
    let aura = run_aura(
        &suite,
        MemoryRetrievalMode::QueryAware,
        "aura-query-aware-v1",
    )
    .await;
    let salience_baseline = run_aura(
        &suite,
        MemoryRetrievalMode::Salience,
        "aura-salience-baseline",
    )
    .await;
    let scope_isolation = evaluate_scope_isolation().await;
    let thresholds = Thresholds {
        minimum_recall_at_k: 0.9,
        minimum_precision_at_k: 0.8,
        maximum_forbidden_recall_rate: 0.0,
        maximum_average_estimated_tokens: 800.0,
    };
    let aura_passed = thresholds.passed_by(&aura, &scope_isolation);
    let aura_improvement_vs_salience = AuraImprovement::between(&aura, &salience_baseline);
    let mut reports = vec![aura, salience_baseline];
    for path in comparison_paths {
        reports.push(score_external(&suite, load_comparison(&path)));
    }

    let report = SuiteReport {
        suite_version: suite.version,
        generated_at: Utc::now().to_rfc3339(),
        thresholds,
        aura_passed,
        aura_improvement_vs_salience,
        scope_isolation,
        reports,
    };
    let json = serde_json::to_string_pretty(&report).expect("serialize benchmark report");
    if let Some(path) = output {
        write_report(&path, &json);
    }
    println!("{json}");

    if !aura_passed {
        std::process::exit(2);
    }
}
