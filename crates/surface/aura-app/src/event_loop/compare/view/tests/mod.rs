//! What `/compare` and the BENCH tab show of a comparison of Gate against
//! Opus, and of it stopped, as a replayed session and as example tasks,
//! pinned as the user sees it.
//!
//! - `summary`: the view of one comparison
//! - `estimate`: the estimate before a comparison starts
//! - `list`: the list of past comparisons
//! - `format`: how amounts, durations, token counts and headlines read
//! - `patch`: a run's diff split per file
//! - `bench`: the BENCH tab's view of a comparison

mod bench;
mod estimate;
mod format;
mod list;
mod patch;
mod summary;

use aura_core_types::{
    AgentId, ArmSpec, BenchSource, CompareArm, CompareCopy, CompareJudgment, ComparePayload,
    CompareRun, CompareStatus, Difficulty, ExampleTaskInfo, ModelTokens, OutcomeSignal, Plan,
    PlanShare, PromptCategory, RefVerdict, RunTurn, SessionBaseline, SystemKind,
};
use std::path::PathBuf;

/// Run `n` of `arm` on `model`, costing `cost`, whose check passed when
/// `check` says so.
fn run(arm: CompareArm, n: u32, model: &str, cost: f64, check: bool) -> CompareRun {
    let id = u8::try_from(n).unwrap() + 0x10 * u8::from(arm == CompareArm::B);
    CompareRun {
        name: format!("{arm}-{n}"),
        arm,
        agent_id: AgentId::new([id; 32]),
        prompt_tx: None,
        model: model.to_string(),
        copy: CompareCopy {
            root: PathBuf::from("/c"),
            workdir: PathBuf::from("/c"),
            git_dir: None,
            baseline: "t".to_string(),
        },
        wall_ms: 40_000 + u64::from(n) * 1_000,
        cost_usd: cost,
        tokens: vec![ModelTokens {
            model: model.trim_start_matches("aura-").to_string(),
            calls: 3,
            input_tokens: 42_000,
            output_tokens: 1_500,
            cost_usd: cost,
            ..ModelTokens::default()
        }],
        plan_usage: Vec::new(),
        turns: Vec::new(),
        check_passed: Some(check),
        files_changed: 2,
        error: None,
        task: None,
    }
}

/// A comparison of Gate against Opus on one prompt, 3 runs each, whose
/// judge preferred Opus's first run.
fn payload() -> ComparePayload {
    let sonnet = "aura-claude-sonnet-4-6";
    let opus = "aura-claude-opus-4-7";
    ComparePayload {
        system_kind: SystemKind::Compare,
        task: "fix the flaky parser test\nmore detail".to_string(),
        plan: None,
        arm_a: ArmSpec::Gate { delegate: true },
        arm_b: ArmSpec::Model {
            model: opus.to_string(),
        },
        against: opus.to_string(),
        prompts: vec!["fix the flaky parser test\nmore detail".to_string()],
        source: BenchSource::Prompt,
        repeats: 3,
        check: Some("cargo test".to_string()),
        budget_usd: None,
        classifier: "aura-claude-haiku-4-5".to_string(),
        requested_by: AgentId::generate(),
        apply_root: PathBuf::from("/repo"),
        started_ms: 0,
        runs: vec![
            run(CompareArm::A, 1, sonnet, 0.05, true),
            run(CompareArm::B, 1, opus, 0.40, true),
            run(CompareArm::A, 2, sonnet, 0.07, true),
            run(CompareArm::B, 2, opus, 0.50, true),
            run(CompareArm::A, 3, sonnet, 0.06, false),
            run(CompareArm::B, 3, opus, 0.45, true),
        ],
        judgments: vec![CompareJudgment {
            a_run: "a-1".to_string(),
            b_run: "b-1".to_string(),
            a_first: true,
            winner: Some(CompareArm::B),
            parsed: true,
            a_vs_reference: None,
            b_vs_reference: None,
            model: opus.to_string(),
            cost_usd: 0.03,
            unpriced_calls: 0,
            reasoning: String::new(),
        }],
        notes: vec!["the budget stopped 2 runs".to_string()],
        status: CompareStatus::Done,
    }
}

/// [`payload`] with no known price for Opus's calls but one run's worth:
/// `b-1` and `b-2` have none priced, `b-3` one call unpriced of three.
fn unpriced() -> ComparePayload {
    let mut payload = payload();
    for run in payload.runs.iter_mut().filter(|r| r.arm == CompareArm::B) {
        let unpriced = if run.name == "b-3" { 1 } else { 3 };
        if unpriced == 3 {
            run.cost_usd = 0.0;
            run.tokens[0].cost_usd = 0.0;
        }
        run.tokens[0].unpriced_calls = unpriced;
    }
    payload
}

/// The cells of the first line of `text` that starts with `start`, split
/// where two spaces part them.
fn line<'a>(text: &'a str, start: &str) -> Vec<&'a str> {
    text.lines()
        .find(|l| l.trim_start().starts_with(start))
        .unwrap_or_else(|| panic!("no {start:?} line in:\n{text}"))
        .split("  ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// `used` of `plan`'s `window` drawn by one run.
fn share(plan: Plan, window: &str, used: f64) -> PlanShare {
    PlanShare {
        plan,
        window: window.to_string(),
        first: 0.1,
        last: 0.1 + used,
        used,
    }
}

/// The example task `id`, titled by its id, with one prompt and a check
/// that passes.
fn example(id: &str) -> ExampleTaskInfo {
    ExampleTaskInfo {
        id: id.to_string(),
        title: id.to_string(),
        prompts: vec![format!("do {id}")],
        check: "true".to_string(),
    }
}

/// [`payload`] whose runs say how their turns were routed, as a
/// comparison the user stopped with a run that stopped too.
fn routed() -> ComparePayload {
    let mut payload = payload();
    payload.runs[0].turns = vec![RunTurn {
        category: Some(PromptCategory::Debugging),
        difficulty: Some(Difficulty::Hard),
        led: Some(false),
        escalations: 1,
        nudges: 1,
        first_model: Some("aura-claude-haiku-4-5".to_string()),
        effort: Some("high".to_string()),
        first_effort: Some("medium".to_string()),
        ..RunTurn::new(
            "aura-claude-sonnet-4-6",
            0.05,
            1,
            OutcomeSignal::ChecksPassed,
        )
    }];
    payload.runs[2].turns = vec![
        RunTurn {
            category: Some(PromptCategory::MultiFileChange),
            led: Some(true),
            subtask_cost_usd: 0.17,
            sent_back: 2,
            ..RunTurn::new("m", 0.77, 1, OutcomeSignal::ChecksPassed)
        },
        RunTurn {
            category: Some(PromptCategory::Question),
            led: Some(false),
            ..RunTurn::new("m", 0.03, 1, OutcomeSignal::JudgedPass)
        },
    ];
    payload.runs[3].error = Some("the model fell over\nand stayed down".to_string());
    payload.runs[3].check_passed = None;
    payload.status = CompareStatus::Cancelled;
    payload
}

/// [`payload`] as a replayed session of two prompts, judged against
/// what the user kept, whose runs reported plan usage.
fn replay() -> ComparePayload {
    let mut replay = payload();
    replay.prompts = vec!["fix the parser".to_string(), "now the docs".to_string()];
    replay.source = BenchSource::Session {
        agent_id: AgentId::generate(),
        started_ms: 1,
        title: "fix the parser".to_string(),
        baseline: SessionBaseline {
            calls: 12,
            cost_usd: 0.9,
            priced: true,
            unpriced_calls: 0,
            models: vec!["claude-opus-4-7".to_string()],
        },
        reference: Some(aura_core_types::Reference {
            diff: "--- a/x\n+++ b/x\n".to_string(),
            reply: "done".to_string(),
        }),
    };
    let turn = |cost: f64, signal: OutcomeSignal| RunTurn::new("", cost, 1, signal);
    for run in &mut replay.runs {
        run.turns = vec![
            turn(0.02, OutcomeSignal::ChecksPassed),
            turn(0.04, OutcomeSignal::Error),
        ];
    }
    replay.judgments[0].a_vs_reference = Some(RefVerdict::Worse);
    replay.judgments[0].b_vs_reference = Some(RefVerdict::Better);
    replay.runs[0].plan_usage = vec![
        share(Plan::Claude, "5h", 0.021),
        share(Plan::Claude, "7d_opus", 0.002),
    ];
    replay.runs[1].plan_usage = vec![share(Plan::Chatgpt, "5h", 0.012)];
    replay
}

/// [`payload`] as two example tasks, the runs taking turns at them.
fn examples() -> ComparePayload {
    let mut bench = payload();
    bench.task = "2 example coding tasks".to_string();
    bench.prompts = Vec::new();
    bench.check = None;
    bench.source = BenchSource::Examples {
        tasks: vec![
            example("coding/write-file"),
            example("coding/fix-shell-bug"),
        ],
    };
    for (i, run) in bench.runs.iter_mut().enumerate() {
        let id = if (i / 2) % 2 == 0 {
            "coding/write-file"
        } else {
            "coding/fix-shell-bug"
        };
        run.task = Some(id.to_string());
    }
    bench
}
