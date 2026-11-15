//! Cost gate mechanics with a scripted mock model: a gate run's
//! coordinator hands a block to a cheaper worker, the block's checks
//! verify it, failures climb the ladder, and every attempt lands in the
//! ledger with its cost.

use std::path::Path;
use std::sync::Arc;

use aura_agent::AgentLoopConfig;
use aura_agent_trace::{RunBundle, RunSource, RunTrace};
use aura_config::TraceLevel;
use aura_model_reasoner::{MockProvider, MockResponse};
use aura_surface_cli::recorded_runs::{self, HeadlessOutcome, HeadlessRun};
use aura_tools::ToolConfig;
use serde_json::{json, Value};

const GATE_MODEL: &str = "gate/claude/claude-opus-5-5";

fn greeting_block() -> MockResponse {
    MockResponse::tool_use(
        "rb1",
        "run_blocks",
        json!({ "blocks": [{
            "title": "write greeting",
            "kind": "edit",
            "risk": "low",
            "instructions": "Create greeting.txt containing exactly `hello, aura`.",
            "files": ["greeting.txt"],
            "checks": ["bash -c \"grep -qx 'hello, aura' greeting.txt\""]
        }]}),
    )
}

fn write(id: &str, content: &str) -> MockResponse {
    MockResponse::tool_use(
        id,
        "write_file",
        json!({ "path": "greeting.txt", "content": content }),
    )
}

async fn gate_run(home: &Path, workspace: &Path, provider: MockProvider) -> HeadlessOutcome {
    let outcome = recorded_runs::run(HeadlessRun {
        home: home.to_path_buf(),
        workspace: workspace.to_path_buf(),
        provider: Arc::new(provider),
        config: AgentLoopConfig::for_agent(GATE_MODEL),
        prompts: vec!["Add a greeting file.".into()],
        meta: aura_agent_trace::RunMeta::new(GATE_MODEL, RunSource::Test),
        level: TraceLevel::Decisions,
        tool_config: ToolConfig::for_autonomous_dev_loop(),
    })
    .await
    .unwrap();
    assert!(outcome.error.is_none(), "{:?}", outcome.error);
    outcome
}

fn ledger(home: &Path) -> Vec<Value> {
    std::fs::read_to_string(home.join("gate/ledger.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn run_blocks_result(home: &Path, run_id: &str) -> Value {
    let trace = RunTrace::load(&RunBundle::find(home, run_id).unwrap()).unwrap();
    let text = serde_json::to_string(&trace.steps).unwrap();
    assert!(
        text.contains("run_blocks"),
        "the coordinator called run_blocks"
    );
    serde_json::to_value(&trace.steps).unwrap()
}

#[tokio::test]
async fn coordinator_hands_a_checked_block_to_the_fast_tier() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let provider = MockProvider::new().with_responses(vec![
        greeting_block(),
        write("w1", "hello, aura\n"),
        MockResponse::text("Wrote greeting.txt."),
        MockResponse::text("Done: the greeting file is in place."),
    ]);
    let outcome = gate_run(home.path(), ws.path(), provider).await;

    assert_eq!(
        std::fs::read_to_string(ws.path().join("greeting.txt")).unwrap(),
        "hello, aura\n"
    );
    let entries = ledger(home.path());
    assert_eq!(entries.len(), 1, "{entries:?}");
    let entry = &entries[0];
    assert_eq!(entry["run_id"], outcome.run_id.as_str());
    assert_eq!(entry["tier"], "fast");
    assert_eq!(entry["model"], "claude/claude-haiku-5-5");
    assert_eq!(entry["passed"], true);
    assert_eq!(entry["level"], "tested");
    assert_eq!(entry["block_passed"], true);

    let scorecard = outcome.scorecard.unwrap();
    assert!(scorecard.child_cost_usd > 0.0, "{scorecard:?}");
    assert!(scorecard.cost_usd > 0.0, "{scorecard:?}");
    run_blocks_result(home.path(), &outcome.run_id);
}

#[tokio::test]
async fn failed_checks_fix_once_then_escalate_from_a_restored_workspace() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("notes.txt"), "keep me\n").unwrap();
    let provider = MockProvider::new().with_responses(vec![
        greeting_block(),
        write("w1", "hi\n"),
        MockResponse::text("Wrote it."),
        write("w2", "hey\n"),
        MockResponse::text("Fixed it."),
        write("w3", "hello, aura\n"),
        MockResponse::text("Wrote greeting.txt."),
        MockResponse::text("Done."),
    ]);
    let outcome = gate_run(home.path(), ws.path(), provider).await;

    assert_eq!(
        std::fs::read_to_string(ws.path().join("greeting.txt")).unwrap(),
        "hello, aura\n"
    );
    assert_eq!(
        std::fs::read_to_string(ws.path().join("notes.txt")).unwrap(),
        "keep me\n"
    );
    let entries = ledger(home.path());
    let tiers: Vec<&str> = entries
        .iter()
        .map(|e| e["tier"].as_str().unwrap())
        .collect();
    let reasons: Vec<&str> = entries
        .iter()
        .map(|e| e["reason"].as_str().unwrap())
        .collect();
    assert_eq!(tiers, ["fast", "fast", "standard"], "{reasons:?}");
    let passed: Vec<bool> = entries
        .iter()
        .map(|e| e["passed"].as_bool().unwrap())
        .collect();
    assert_eq!(passed, [false, false, true]);
    assert!(entries.iter().all(|e| e["block_passed"] == true));
    assert_eq!(entries[2]["model"], "claude/claude-sonnet-5-5");
    assert!(outcome.scorecard.unwrap().child_cost_usd > 0.0);
}
