//! `aura eval run`'s runs under Gate: routed and judged as `aura -p`'s
//! turns are, each task's check run through the run's own agent, and its
//! verdict recorded and counted in Gate's table, on the attempt that
//! finished the work.

use super::{
    dirs, failing_check_provider, harness, models, record_of, sonnet_and_opus, sonnet_only, Dirs,
    Requests, GATE_HAIKU, GATE_OPUS, GATE_SONNET,
};
use crate::agent_registry::RegisteredAgent;
use crate::event_loop::subagents::lock;
use crate::event_loop::{AgentHub, GateBase};
use crate::headless::tools::ToolFilter;
use crate::headless::{EvalRan, EvalTurn, EvalTurns};
use aura_agent_trace::{RunBundle, RunMeta, RunRecorder, RunScorecard, RunSource, RunTrace};
use aura_core_types::{OutcomeSignal, PromptCategory, Route, RouteLevel, RouteReason};
use aura_core_types::{ToolProposal, TransactionType};
use aura_model::{MockProvider, MockResponse};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

/// An eval task: its id, its prompt and its verify command.
type Task<'a> = (&'a str, &'a str, &'a str);

/// What an eval run left: how it went, its trace's id and its card.
type Ran = (EvalRan, String, RunScorecard);

/// Eval runs on a headless harness, with the hub they share, every
/// request their model was sent, and where their traces and workspaces
/// are kept.
struct Evals {
    turns: EvalTurns,
    hub: Arc<AgentHub>,
    requests: Requests,
    home: TempDir,
    work: TempDir,
    _dirs: Dirs,
}

impl Evals {
    /// Eval runs under `gate`, their model answering with `provider`.
    fn on(gate: GateBase, provider: MockProvider) -> Self {
        let dirs = dirs();
        let permissions = crate::headless::permissions(aura_core_modes::AgentMode::Agent, None);
        let (h, requests) = harness(&dirs, provider, ToolFilter::default(), permissions, gate);
        Self {
            hub: Arc::clone(&h.hub),
            turns: EvalTurns::new(h),
            requests,
            home: TempDir::new().unwrap(),
            work: TempDir::new().unwrap(),
            _dirs: dirs,
        }
    }

    /// Run `task` in `workspace`, traced under [`Self::home`].
    async fn run(&mut self, (id, prompt, verify): Task<'_>, workspace: &Path) -> Ran {
        let mut meta = RunMeta::new("gate", RunSource::Eval);
        meta.task_id = Some(format!("s/{id}"));
        let level = aura_config::trace::TraceLevel::Full;
        let trace = RunRecorder::start(self.home.path(), meta, level)
            .unwrap()
            .unwrap();
        let name = format!("Eval s/{id}");
        let turn = EvalTurn {
            name: &name,
            workspace,
            prompts: &[prompt.to_string()],
            max_iterations: None,
            checks: None,
            verify,
            trace: Arc::clone(&trace),
        };
        let ran = tokio::time::timeout(Duration::from_secs(60), self.turns.run(turn))
            .await
            .expect("the eval run did not finish")
            .unwrap();
        let card = trace.finish().unwrap();
        (ran, trace.run_id(), card)
    }

    /// The runs' agents, by name.
    fn agents(&self) -> Vec<RegisteredAgent> {
        (lock(&self.hub.registry).agents().iter())
            .filter(|a| a.name.starts_with("Eval s/"))
            .cloned()
            .collect()
    }

    /// Tries and successes of `model`'s row of `category` in Gate's
    /// table.
    fn row(&self, category: PromptCategory, model: &str) -> Option<(u32, u32)> {
        let stats = self.hub.gate.stats();
        (stats.row(RouteLevel::Turn, category, model)).map(|row| (row.tries, row.successes))
    }
}

/// Two eval runs under [`sonnet_only`] of questions in workspaces
/// holding `note.txt`: the first task's check fails, the second's
/// passes.
async fn two_tasks() -> (Evals, Vec<Ran>) {
    let answer = MockResponse::text("question medium");
    let mut evals = Evals::on(
        sonnet_only(),
        MockProvider::new().with_default_response(answer),
    );
    let tasks = [
        ("fails", "is there a missing.txt?", "test -f missing.txt"),
        (
            "passes",
            "what does note.txt say?",
            "[ \"$(cat note.txt)\" = 'hi' ]",
        ),
    ];
    let mut runs = Vec::new();
    for task in tasks {
        let workspace = evals.work.path().join(task.0);
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("note.txt"), "hi\n").unwrap();
        let workspace = workspace.canonicalize().unwrap();
        runs.push(evals.run(task, &workspace).await);
    }
    (evals, runs)
}

#[tokio::test]
async fn an_eval_run_under_gate_is_routed_and_judged() {
    let (evals, runs) = two_tasks().await;

    let passed: Vec<Option<bool>> = runs.iter().map(|(ran, _, _)| ran.check.passed).collect();
    assert_eq!(passed, [Some(false), Some(true)]);
    assert_eq!(
        models(&evals.requests),
        [
            GATE_HAIKU,
            GATE_SONNET,
            GATE_SONNET,
            GATE_HAIKU,
            GATE_SONNET,
            GATE_SONNET
        ],
        "each task classified, routed, then judged, as its turn ran no check"
    );
    for (ran, _, _) in &runs {
        assert_eq!(ran.gate.routed, [GATE_SONNET]);
        let calls: u32 = ran.gate.tokens.iter().map(|m| m.calls).sum();
        assert_eq!(calls, 3, "the classifier call, the turn and the judge's");
        assert!(ran.error.is_none(), "{:?}", ran.error);
    }
}

#[tokio::test]
async fn an_eval_runs_verdict_is_recorded_after_its_turns_outcome() {
    let (evals, _) = two_tasks().await;

    let agents = evals.agents();
    assert_eq!(agents.len(), 2);
    let verdicts = [OutcomeSignal::ChecksFailed, OutcomeSignal::ChecksPassed];
    for (agent, verdict) in agents.iter().zip(verdicts) {
        let entries = record_of(&evals.hub, agent.agent_id());
        let prompts: Vec<_> = entries
            .iter()
            .filter_map(|e| e.tx.prompt_payload())
            .collect();
        assert_eq!(prompts.len(), 1, "{}", agent.name);
        assert_eq!(prompts[0].category, Some(PromptCategory::Question));
        assert_eq!(
            prompts[0].route,
            Route::turn(GATE_SONNET, RouteReason::Default).at_effort("medium")
        );
        let signals: Vec<OutcomeSignal> = (entries.iter())
            .filter_map(|e| e.tx.outcome_payload())
            .map(|o| o.signal)
            .collect();
        let unavailable = OutcomeSignal::Unavailable;
        assert_eq!(signals, [unavailable, verdict], "{}", agent.name);
        let verify = (entries.iter())
            .filter(|e| e.tx.tx_type == TransactionType::ToolProposal)
            .filter_map(|e| serde_json::from_slice::<ToolProposal>(&e.tx.payload).ok())
            .find(|p| p.source.as_deref() == Some("eval_verify"))
            .expect("the verify command went through the agent's kernel");
        assert_eq!(verify.tool, "run_command");
    }
}

#[tokio::test]
async fn eval_runs_under_gate_count_in_its_table() {
    let (evals, _) = two_tasks().await;

    assert_eq!(
        evals.row(PromptCategory::Question, GATE_SONNET),
        Some((2, 1)),
        "the eval runs count in Gate's table"
    );
}

#[tokio::test]
async fn an_eval_runs_routed_turn_is_in_its_trace() {
    let (evals, runs) = two_tasks().await;

    let (_, run_id, card) = &runs[1];
    let trace = RunTrace::load(&RunBundle::find(evals.home.path(), run_id).unwrap()).unwrap();
    assert_eq!(trace.meta.model, "gate");
    assert_eq!(card.model_calls, 1, "the routed turn is in the run's trace");
}

/// An eval run under [`sonnet_and_opus`] whose check fails on Sonnet,
/// which the judge blames, so Opus takes the work over and finishes it;
/// the task's own check then passes.
async fn moved_up() -> (Evals, EvalRan) {
    let then = ["It skips a token.\nVERDICT: MODEL", "Fixed it."];
    let mut evals = Evals::on(sonnet_and_opus(), failing_check_provider(&then));
    let workspace = evals.work.path().canonicalize().unwrap();
    let task = ("parser", "fix the parser", "test -f parser.txt");
    let (ran, _, _) = evals.run(task, &workspace).await;
    (evals, ran)
}

#[tokio::test]
async fn an_eval_run_that_moved_up_reports_its_judge_and_every_call() {
    let (evals, ran) = moved_up().await;

    assert!(ran.error.is_none(), "{:?}", ran.error);
    assert_eq!(ran.check.passed, Some(true));
    assert_eq!(ran.gate.routed, [GATE_SONNET, GATE_OPUS]);
    assert_eq!(ran.gate.judged.len(), 1);
    let judged = &ran.gate.judged[0];
    assert_eq!(
        (judged.model.as_str(), judged.judge.as_str()),
        (GATE_SONNET, GATE_OPUS)
    );
    assert!(judged.verdict.at_fault);
    assert_eq!(judged.verdict.reasons, "It skips a token.");
    let asked = models(&evals.requests);
    let recorded: u32 = ran.gate.tokens.iter().map(|m| m.calls).sum();
    assert_eq!(
        recorded as usize,
        asked.len(),
        "every model call is in the run's cost, the judge's included: {asked:?}"
    );
    assert_eq!(
        asked.iter().filter(|model| *model == GATE_OPUS).count(),
        3,
        "the judge's call, the turn it sent on, and the judge's on that turn's \
         unchecked work: {asked:?}"
    );
}

#[tokio::test]
async fn an_eval_verdict_after_a_move_up_lands_on_the_uncounted_attempt() {
    let (evals, _) = moved_up().await;

    let agent = evals
        .agents()
        .iter()
        .find(|a| a.name == "Eval s/parser")
        .unwrap()
        .agent_id();
    let outcomes: Vec<(String, OutcomeSignal)> = (record_of(&evals.hub, agent).iter())
        .filter_map(|e| e.tx.outcome_payload())
        .map(|o| (o.model, o.signal))
        .collect();
    assert_eq!(
        outcomes,
        [
            (GATE_SONNET.to_string(), OutcomeSignal::ChecksFailed),
            (GATE_SONNET.to_string(), OutcomeSignal::JudgedFail),
            (GATE_OPUS.to_string(), OutcomeSignal::Unavailable),
            (GATE_OPUS.to_string(), OutcomeSignal::ChecksPassed),
        ],
        "the hidden check's verdict is recorded for the model that finished the work"
    );
    let debugging = PromptCategory::Debugging;
    assert_eq!(
        evals.row(debugging, GATE_SONNET),
        Some((1, 0)),
        "the start model's failure stands"
    );
    assert_eq!(
        evals.row(debugging, GATE_OPUS),
        None,
        "work taken over is not counted"
    );
}
