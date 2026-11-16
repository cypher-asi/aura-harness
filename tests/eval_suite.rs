//! Eval suite and branch mechanics with a scripted mock model: workspace
//! setup, recording, verification, scoring, arm comparison, and
//! replaying a recorded prefix before continuing live.

use std::path::Path;
use std::sync::Arc;

use aura_agent::AgentLoopConfig;
use aura_agent_trace::{RunBundle, RunSource, RunTrace};
use aura_config::TraceLevel;
use aura_core_types::trace::StepAction;
use aura_model_reasoner::{MockProvider, MockResponse, ModelProvider};
use aura_app::recorded_runs::{self, HeadlessRun};
use aura_cli::evals::{self, EvalOptions};
use aura_tools::ToolConfig;
use serde_json::json;

const SUITE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/evals/coding");

fn write_greeting() -> MockProvider {
    MockProvider::new().with_responses(vec![
        MockResponse::tool_use(
            "w1",
            "write_file",
            json!({ "path": "greeting.txt", "content": "hello, aura\n" }),
        ),
        MockResponse::text("Created greeting.txt."),
    ])
}

fn options(home: &Path, arm: &str, provider: fn() -> MockProvider) -> EvalOptions {
    EvalOptions {
        home: home.to_path_buf(),
        suite: "coding".into(),
        arm: Some(arm.into()),
        repeat: 1,
        level: TraceLevel::Full,
        config: AgentLoopConfig::for_agent("mock-model"),
        tool_config: ToolConfig::for_autonomous_dev_loop(),
        work_root: home.join("work"),
        provider: Box::new(move |_| Arc::new(provider())),
    }
}

#[tokio::test]
async fn eval_runs_are_verified_scored_and_compared() {
    let home = tempfile::tempdir().unwrap();
    let mut tasks = evals::load_suite(Path::new(SUITE)).unwrap();
    assert!(tasks.len() >= 5, "suite has {} tasks", tasks.len());
    tasks.retain(|t| t.id == "write-file");

    let good = evals::run_suite(&tasks, &options(home.path(), "good", write_greeting))
        .await
        .unwrap();
    assert_eq!(good.len(), 1);
    assert!(good[0].passed, "{good:?}");
    assert_eq!(good[0].scorecard.outcome, Some(1.0));
    assert_eq!(good[0].scorecard.writes, 1);

    let bundle = RunBundle::find(home.path(), &good[0].run_id).unwrap();
    let trace = RunTrace::load(&bundle).unwrap();
    assert_eq!(trace.meta.source, RunSource::Eval);
    assert_eq!(trace.meta.task_id.as_deref(), Some("coding/write-file"));
    assert_eq!(trace.meta.arm.as_deref(), Some("good"));
    assert!(bundle.dir().join("record").is_dir());
    let linked: Vec<u64> = trace
        .steps
        .iter()
        .filter(|s| !s.links.record_seqs.is_empty())
        .map(|s| s.seq)
        .collect();
    assert!(
        trace
            .steps
            .iter()
            .any(|s| matches!(s.action, StepAction::ToolResult { .. })
                && !s.links.record_seqs.is_empty()),
        "tool results are joined to the kernel record (linked steps: {linked:?})"
    );

    let lazy = || MockProvider::simple_response("I'd rather not.");
    let bad = evals::run_suite(&tasks, &options(home.path(), "lazy", lazy))
        .await
        .unwrap();
    assert!(!bad[0].passed);
    assert_eq!(bad[0].scorecard.outcome, Some(0.0));
    assert!(evals::results_table(&bad).contains("0/1 passed"));

    let runs = evals::runs_by_arm(home.path(), Some("coding"));
    assert_eq!(runs.keys().collect::<Vec<_>>(), ["good", "lazy"]);
    let (report, regressions) =
        evals::compare(&runs, &["good".into(), "lazy".into()], 0.1).unwrap();
    assert!(regressions >= 1, "{report}");
    assert!(report.contains("regression: lazy outcome"), "{report}");
    assert!(report.contains("coding/write-file"), "{report}");
    assert!(evals::compare(&runs, &["missing".into()], 0.1).is_err());

    let path = home.path().join("baseline.json");
    evals::Baseline::from_runs(&runs, "good", Some("coding"))
        .unwrap()
        .save(&path)
        .unwrap();
    let baseline = evals::Baseline::load(&path).unwrap();
    assert_eq!(baseline.tasks["coding/write-file"], [1, 1]);
    let (report, regressions) = baseline.compare(&runs, &["good".into()], 0.1).unwrap();
    assert_eq!(regressions, 0, "{report}");
    let (report, regressions) = baseline.compare(&runs, &["lazy".into()], 0.1).unwrap();
    assert!(
        report.contains("regression: coding/write-file pass rate 1.00 → 0.00"),
        "{report}"
    );
    assert!(regressions >= 2, "{report}");
}

#[tokio::test]
async fn branch_replays_the_prefix_then_runs_live() {
    let home = tempfile::tempdir().unwrap();
    let original_ws = tempfile::tempdir().unwrap();
    let original = recorded_runs::run(HeadlessRun {
        home: home.path().to_path_buf(),
        workspace: original_ws.path().to_path_buf(),
        provider: Arc::new(write_greeting()),
        config: AgentLoopConfig::for_agent("mock-model"),
        prompts: vec!["Write greeting.txt".into()],
        meta: aura_agent_trace::RunMeta::new("mock-model", RunSource::Test),
        level: TraceLevel::Full,
        tool_config: ToolConfig::for_autonomous_dev_loop(),
    })
    .await
    .unwrap();
    assert!(original.error.is_none(), "{:?}", original.error);
    let source = RunTrace::load(&RunBundle::find(home.path(), &original.run_id).unwrap()).unwrap();
    let model_steps: Vec<u64> = source
        .steps
        .iter()
        .filter(|s| matches!(s.action, StepAction::Model(_)))
        .map(|s| s.seq)
        .collect();
    assert_eq!(model_steps.len(), 2);

    let from = model_steps[1];
    let plan = recorded_runs::plan_branch(&source, from).unwrap();
    assert_eq!(plan.prompts, ["Write greeting.txt"]);
    let live: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("A different ending."));
    let provider = recorded_runs::replay_provider(home.path(), &source, from, live).unwrap();
    let branch_ws = tempfile::tempdir().unwrap();
    let branch = recorded_runs::run(HeadlessRun {
        home: home.path().to_path_buf(),
        workspace: branch_ws.path().to_path_buf(),
        provider,
        config: AgentLoopConfig::for_agent("mock-model"),
        prompts: plan.prompts,
        meta: plan.meta,
        level: TraceLevel::Full,
        tool_config: ToolConfig::for_autonomous_dev_loop(),
    })
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(branch_ws.path().join("greeting.txt")).unwrap(),
        "hello, aura\n",
        "the replayed tool call ran in the branch workspace"
    );
    let trace = RunTrace::load(&RunBundle::find(home.path(), &branch.run_id).unwrap()).unwrap();
    assert_eq!(trace.meta.source, RunSource::Branch);
    assert_eq!(
        trace.meta.parent_run.as_deref(),
        Some(original.run_id.as_str())
    );
    assert_eq!(trace.meta.branch_step, Some(from));
    let last_text = trace
        .steps
        .iter()
        .rev()
        .find_map(|s| match &s.action {
            StepAction::Model(m) => Some(m.text.clone()),
            _ => None,
        })
        .unwrap();
    assert!(last_text.contains("different ending"), "{last_text}");
}
