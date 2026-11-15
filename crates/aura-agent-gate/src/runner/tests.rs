use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use aura_core_types::{AgentPermissions, SubagentDispatchRequest, SubagentResult};
use aura_tools::{Sandbox, SubagentDispatchHook, ToolConfig, ToolContext};

use super::*;
use crate::block::tests::edit;

/// Workers on `good` models write `fixed`, others write `broken`;
/// reviewers approve.
struct FakeWorkers {
    root: PathBuf,
    good: Vec<&'static str>,
    calls: Mutex<Vec<SubagentDispatchRequest>>,
}

#[async_trait]
impl SubagentDispatchHook for FakeWorkers {
    async fn dispatch(&self, request: SubagentDispatchRequest) -> Result<SubagentResult, String> {
        let model = request.model_override.clone().unwrap_or_default();
        let reviewer = request
            .override_tool_subset
            .as_ref()
            .is_some_and(|t| !t.iter().any(|n| n == "edit_file"));
        lock(&self.calls).push(request);
        let mut result = SubagentResult::completed(AgentId::generate(), "APPROVE\nok");
        result.cost_micros = Some(1_000);
        if !reviewer {
            let text = if self.good.contains(&model.as_str()) {
                "fixed"
            } else {
                "broken"
            };
            std::fs::write(self.root.join("out.txt"), text).unwrap();
            result.files_changed = vec!["out.txt".into()];
            result.final_message = format!("wrote {text}");
        }
        Ok(result)
    }
}

fn setup(good: Vec<&'static str>) -> (tempfile::TempDir, Arc<FakeWorkers>, ToolContext, AgentId) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("out.txt"), "orig").unwrap();
    let workers = Arc::new(FakeWorkers {
        root: dir.path().to_path_buf(),
        good,
        calls: Mutex::new(Vec::new()),
    });
    let mut config = ToolConfig::default();
    config.command.enabled = true;
    config.command.allow_shell = true;
    config.command.bypass_allowlists = true;
    let mut ctx = ToolContext::new(Sandbox::new(dir.path()).unwrap(), config);
    let agent = AgentId::generate();
    ctx.caller_agent_id = Some(agent);
    ctx.caller_permissions = Some(AgentPermissions::full_access());
    ctx.subagent_dispatch = Some(workers.clone() as Arc<dyn SubagentDispatchHook>);
    (dir, workers, ctx, agent)
}

fn checked_edit(title: &str) -> BlockSpec {
    let mut spec = edit(title);
    spec.files = vec!["out.txt".into()];
    spec.checks = vec!["grep -qx fixed out.txt".into()];
    spec
}

fn runner(ledger: Option<PathBuf>) -> BlockRunner {
    BlockRunner::new(GateSettings {
        ledger,
        run_id: Some("run-1".into()),
        ..GateSettings::default()
    })
}

fn read(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("out.txt"))
        .unwrap()
        .trim_end()
        .to_string()
}

#[tokio::test]
async fn passing_on_the_rule_tier_takes_one_worker() {
    let (dir, workers, ctx, agent) = setup(vec!["claude-sonnet-5-5"]);
    let runner = runner(None);
    runner.set_frontier(agent, "gate/claude-opus-5-5");
    let out = runner.run(&ctx, &[checked_edit("fix")]).await.unwrap();
    assert_eq!(out.passed, 1);
    let block = &out.blocks[0];
    assert_eq!(block.tier, Some(Tier::Standard));
    assert_eq!(block.attempts.len(), 1);
    assert_eq!(block.attempts[0].reason, "rule");
    assert!((out.cost_usd - 0.001).abs() < 1e-9);
    assert_eq!(read(dir.path()), "fixed");
    let calls = lock(&workers.calls);
    assert_eq!(calls[0].subagent_type, GATE_BLOCK_AGENT);
    assert_eq!(
        calls[0].reasoning_effort_override.as_deref(),
        Some("medium")
    );
}

#[tokio::test]
async fn failures_fix_then_escalate_from_restored_files() {
    let (dir, workers, ctx, agent) = setup(vec!["claude-opus-5-5"]);
    let runner = runner(None);
    runner.set_frontier(agent, "claude-opus-5-5");
    let out = runner.run(&ctx, &[checked_edit("fix")]).await.unwrap();
    let block = &out.blocks[0];
    assert_eq!(block.status, BlockStatus::Passed);
    let reasons: Vec<&str> = block.attempts.iter().map(|a| a.reason.as_str()).collect();
    assert_eq!(reasons, ["rule", "fix", "escalate"]);
    assert_eq!(block.tier, Some(Tier::Frontier));
    assert_eq!(read(dir.path()), "fixed");
    let calls = lock(&workers.calls);
    assert!(calls[1].prompt.contains("Its changes are still in place"));
    assert!(calls[2].prompt.contains("rolled back"));
    assert!(calls[2].prompt.contains("grep -qx fixed out.txt"));
}

#[tokio::test]
async fn a_failed_block_is_rolled_back_and_stops_the_rest() {
    let dir_ledger = tempfile::tempdir().unwrap();
    let ledger = dir_ledger.path().join("ledger.jsonl");
    let (dir, _workers, ctx, agent) = setup(vec![]);
    let runner = runner(Some(ledger.clone()));
    runner.set_frontier(agent, "claude-opus-5-5");
    let out = runner
        .run(&ctx, &[checked_edit("first"), checked_edit("second")])
        .await
        .unwrap();
    assert_eq!((out.passed, out.failed, out.skipped), (0, 1, 1));
    let block = &out.blocks[0];
    assert!(block.restored);
    assert!(block.files_changed.is_empty());
    assert_eq!(read(dir.path()), "orig");
    assert_eq!(block.attempts.len(), 4, "standard, fix, frontier, fix");

    let entries = Ledger::read(&ledger).unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0].run_id.as_deref(), Some("run-1"));
    assert!(entries.iter().all(|e| !e.block_passed));
}

#[tokio::test]
async fn review_adds_a_level_and_its_cost() {
    let (_dir, workers, ctx, agent) = setup(vec!["claude-sonnet-5-5"]);
    let runner = runner(None);
    runner.set_frontier(agent, "claude-opus-5-5");
    let mut spec = checked_edit("fix");
    spec.review = true;
    let out = runner.run(&ctx, &[spec]).await.unwrap();
    assert_eq!(out.blocks[0].level, VerifyLevel::Reviewed);
    assert!((out.cost_usd - 0.002).abs() < 1e-9);
    assert_eq!(lock(&workers.calls).len(), 2);
}

#[tokio::test]
async fn command_blocks_run_without_a_model() {
    let (dir, workers, ctx, agent) = setup(vec![]);
    let runner = runner(None);
    runner.set_frontier(agent, "claude-opus-5-5");
    let spec: BlockSpec = serde_json::from_value(serde_json::json!({
        "title": "write", "kind": "command", "command": "echo fixed > out.txt",
        "checks": ["grep -qx fixed out.txt"]
    }))
    .unwrap();
    let out = runner.run(&ctx, &[spec]).await.unwrap();
    assert_eq!(out.passed, 1);
    assert_eq!(out.blocks[0].level, VerifyLevel::Tested);
    assert_eq!(read(dir.path()), "fixed");
    assert!(lock(&workers.calls).is_empty());
}

#[tokio::test]
async fn bad_input_fails_before_running() {
    let (_dir, _workers, ctx, agent) = setup(vec![]);
    let runner = runner(None);
    assert!(
        runner.run(&ctx, &[checked_edit("a")]).await.is_err(),
        "no frontier"
    );
    runner.set_frontier(agent, "claude-opus-5-5");
    assert!(runner.run(&ctx, &[]).await.is_err());
    let dup = [checked_edit("a"), checked_edit("a")];
    assert!(runner
        .run(&ctx, &dup)
        .await
        .unwrap_err()
        .contains("two blocks"));
}
