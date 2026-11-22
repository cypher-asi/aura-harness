//! Gate against the benchmark runs recorded on `swe_bench_pro_hard`.
//!
//! [`RUNS`] and [`LAST`] are the runs of five recorded `/compare`
//! benchmarks of Gate against Claude Opus 5.5, as the record has them:
//! the task, the model that did it, how its prompt was classified at the
//! time, what the turn cost and whether the task's own check passed.
//!
//! Two things went wrong in them. In #19 Gate sent hard work to Claude
//! Sonnet 5, an older model it had never tried on such work, because it
//! lists at half of Opus's price per token. It then used several times
//! the tokens: 2.6 times Opus's cost on one task and 1.9 times over the
//! three it ran. Gate no longer sends work to a model a later version
//! supersedes.
//!
//! Claude Sonnet 5.5, which had done these tasks for about a third of
//! Opus's cost and passed nine of ten, was then ruled out of hard
//! multi-file work by two failed turns on an easy task, so Gate did all
//! such work on Opus. A standard model is now judged on work of the same
//! difficulty, and keeps the work until its outcomes show that starting
//! on it costs more than starting on Opus.
//!
//! The test replays the benchmark's tasks on what Gate has recorded, and
//! holds Gate to the result of the strongest model at less than its
//! cost: the tasks start on Sonnet 5.5, and the one whose recorded
//! Sonnet run failed its check goes on to Opus.

use aura_agent_gate::router::{candidates, current, escalate, pick, Candidate, Work};
use aura_agent_gate::GateStats;
use aura_config::GateConfig;
use aura_core_auth::ProviderKind;
use aura_core_types::{
    AgentId, Difficulty, OutcomePayload, OutcomeSignal, PromptCategory, PromptPayload, RecordEntry,
    Route, RouteLevel, RouteReason, Transaction,
};
use aura_model::model_catalog::{infer_tier, ModelBackend, ModelOption};

const HAIKU: &str = "claude/claude-haiku-5-5";
const SONNET: &str = "claude/claude-sonnet-5-5";
const OLD_SONNET: &str = "claude/claude-sonnet-5";
const OPUS: &str = "claude/claude-opus-5-5";

/// How a run's prompt was classified: not at all (the classifier failed
/// on long prompts at first, and such outcomes no longer count), or as
/// hard work of a category.
#[derive(Clone, Copy)]
enum Kind {
    U,
    DH,
    MH,
}
use Kind::{DH, MH, U};

impl Kind {
    const fn work(self) -> Option<(PromptCategory, Option<Difficulty>)> {
        match self {
            U => None,
            DH => Some((PromptCategory::Debugging, Some(Difficulty::Hard))),
            MH => Some((PromptCategory::MultiFileChange, Some(Difficulty::Hard))),
        }
    }
}

/// A recorded run: the benchmark it belongs to, its task, its model, how
/// its prompt was classified, the turn's cost in USD and whether the
/// task's check passed.
type Run = (u8, &'static str, &'static str, Kind, f64, bool);

/// The benchmarks before #19, in the order they ran.
const RUNS: &[Run] = &[
    (14, "ansible-40ade1f8", SONNET, U, 0.317, true),
    (14, "ansible-40ade1f8", OPUS, U, 0.895, true),
    (14, "ansible-b748edea", HAIKU, DH, 0.015, true),
    (14, "ansible-b748edea", OPUS, DH, 0.219, true),
    (14, "ansible-c1f2df47", SONNET, U, 0.108, true),
    (14, "ansible-c1f2df47", OPUS, U, 0.333, true),
    (14, "ansible-d33bedc4", SONNET, U, 0.103, true),
    (14, "ansible-d33bedc4", OPUS, U, 0.553, true),
    (14, "ansible-d62496fe", HAIKU, DH, 0.011, true),
    (14, "ansible-d62496fe", OPUS, U, 0.169, true),
    (16, "ansible-40ade1f8", SONNET, U, 0.350, true),
    (16, "ansible-40ade1f8", OPUS, U, 0.936, true),
    (16, "ansible-b748edea", HAIKU, DH, 0.015, true),
    (16, "ansible-b748edea", OPUS, DH, 0.208, true),
    (16, "ansible-c1f2df47", SONNET, U, 0.109, true),
    (16, "ansible-c1f2df47", OPUS, U, 0.400, true),
    (16, "ansible-d33bedc4", SONNET, U, 0.083, false),
    (16, "ansible-d33bedc4", OPUS, U, 0.483, true),
    (16, "ansible-d62496fe", HAIKU, DH, 0.008, true),
    (16, "ansible-d62496fe", OPUS, DH, 0.161, true),
    (16, "ansible-d72025be", OLD_SONNET, U, 1.504, true),
    (16, "ansible-d72025be", OPUS, U, 1.098, true),
    (16, "ansible-de5858f4", HAIKU, DH, 0.024, true),
    (16, "ansible-de5858f4", OPUS, DH, 0.453, true),
    (16, "ansible-e9e60012", HAIKU, DH, 0.018, true),
    (16, "ansible-e9e60012", OPUS, DH, 0.252, true),
    (16, "element-web-5dfde12c", OLD_SONNET, U, 0.907, true),
    (16, "element-web-5dfde12c", OPUS, DH, 0.373, true),
    (16, "element-web-66d0b318", OLD_SONNET, U, 1.037, false),
    (16, "element-web-66d0b318", OPUS, U, 0.558, true),
    (16, "element-web-772df302", SONNET, U, 0.167, true),
    (16, "element-web-772df302", OPUS, U, 0.356, true),
    (16, "element-web-ecfd1736", SONNET, U, 0.078, true),
    (16, "element-web-ecfd1736", OPUS, U, 0.233, true),
    (16, "flipt-2ca5dfb3", SONNET, U, 0.238, true),
    (16, "flipt-2ca5dfb3", OPUS, U, 1.092, true),
    (16, "flipt-3d5a345f", OPUS, U, 1.071, true),
    (16, "flipt-3d5a345f", OPUS, U, 0.671, true),
    (16, "flipt-524f2773", OPUS, U, 0.548, true),
    (16, "flipt-524f2773", OPUS, U, 0.630, true),
    (16, "flipt-c1728053", OPUS, U, 0.224, true),
    (16, "flipt-c1728053", OPUS, U, 0.216, true),
    (16, "flipt-c188284f", OPUS, U, 0.968, true),
    (16, "flipt-c188284f", OPUS, U, 0.648, true),
    (16, "flipt-cd18e54a", OPUS, U, 0.922, true),
    (16, "flipt-cd18e54a", OPUS, U, 0.978, true),
    (16, "flipt-e2bd19da", OPUS, U, 0.381, true),
    (16, "flipt-e2bd19da", OPUS, U, 0.290, true),
    (16, "navidrome-69e0a266", OPUS, U, 0.311, false),
    (16, "navidrome-69e0a266", OPUS, U, 0.303, false),
    (17, "ansible-40ade1f8", SONNET, MH, 0.339, true),
    (17, "ansible-40ade1f8", OPUS, MH, 0.887, true),
    (17, "ansible-b748edea", HAIKU, MH, 0.015, false),
    (17, "ansible-b748edea", OPUS, MH, 0.206, true),
    (17, "ansible-c1f2df47", HAIKU, MH, 0.013, false),
    (17, "ansible-c1f2df47", OPUS, MH, 0.402, true),
    (17, "ansible-d33bedc4", HAIKU, MH, 0.017, true),
    (17, "ansible-d33bedc4", OPUS, MH, 1.130, true),
    (17, "ansible-d62496fe", HAIKU, MH, 0.007, true),
    (17, "ansible-d62496fe", OPUS, MH, 0.259, true),
    (18, "ansible-b748edea", OPUS, MH, 0.176, true),
    (18, "ansible-b748edea", OPUS, MH, 0.484, true),
    (18, "ansible-40ade1f8", OPUS, MH, 0.889, true),
    (18, "ansible-c1f2df47", OPUS, MH, 0.486, true),
    (18, "ansible-c1f2df47", OPUS, MH, 0.356, true),
    (18, "ansible-40ade1f8", OPUS, MH, 1.096, true),
    (18, "ansible-d33bedc4", OPUS, MH, 0.363, true),
];

/// The runs of benchmark #19.
const LAST: &[Run] = &[
    (19, "ansible-b748edea", OPUS, MH, 0.220, true),
    (19, "ansible-b748edea", OLD_SONNET, MH, 0.579, true),
    (19, "ansible-c1f2df47", OLD_SONNET, MH, 0.489, true),
    (19, "ansible-c1f2df47", OPUS, MH, 0.377, true),
    (19, "ansible-40ade1f8", OPUS, MH, 1.057, true),
    (19, "ansible-d62496fe", OPUS, U, 0.211, true),
    (19, "ansible-d33bedc4", OPUS, MH, 0.542, true),
    (19, "ansible-d62496fe", OPUS, MH, 0.186, true),
    (19, "ansible-40ade1f8", OLD_SONNET, MH, 2.167, true),
];

/// The tasks of benchmark #19 that both Sonnet 5.5 and Opus 5.5 have a
/// recorded run on. The record cannot say how the other two would go on
/// Sonnet 5.5.
const TASKS: [&str; 3] = ["ansible-40ade1f8", "ansible-c1f2df47", "ansible-d33bedc4"];

/// The Claude plan models Gate routed among.
fn plan() -> Vec<Candidate> {
    let options: Vec<ModelOption> = [
        "claude-fable-5-2",
        "claude-haiku-5-5",
        "claude-sonnet-5-5",
        "claude-opus-5-5",
        "claude-fable-5-1",
        "claude-opus-5",
        "claude-sonnet-5",
        "claude-fable-5",
        "claude-opus-4-8",
        "claude-opus-4-7",
        "claude-sonnet-4-6",
        "claude-opus-4-6",
        "claude-opus-4-5",
        "claude-haiku-4-5",
        "claude-sonnet-4-5",
    ]
    .into_iter()
    .map(|name| ModelOption {
        id: format!("claude/{name}"),
        label: name.to_string(),
        backend: ModelBackend::Claude,
        installed: false,
        tier: infer_tier(name),
    })
    .collect();
    candidates(
        &options,
        |provider| provider == ProviderKind::Claude,
        |_| false,
    )
}

/// A record of turns, as a benchmark writes them.
#[derive(Default)]
struct Log {
    entries: Vec<RecordEntry>,
}

impl Log {
    fn push(&mut self, tx: Transaction) {
        let seq = u64::try_from(self.entries.len()).unwrap() + 1;
        self.entries.push(RecordEntry::builder(seq, tx).build());
    }

    /// A turn on `model` that the agent did itself, with its check's
    /// verdict.
    fn turn(
        &mut self,
        model: &str,
        (category, difficulty): (PromptCategory, Option<Difficulty>),
        cost_usd: f64,
        passed: bool,
    ) {
        let agent = AgentId::generate();
        let mut prompt = PromptPayload::new(format!("task {}", self.entries.len()));
        prompt.category = Some(category);
        prompt.difficulty = difficulty;
        prompt.led = Some(false);
        prompt.route = Route::turn(model, RouteReason::Compare);
        let tx = Transaction::user_prompt_with(agent, &prompt);
        let prompt_tx = tx.hash;
        self.push(tx);
        let signal = if passed {
            OutcomeSignal::ChecksPassed
        } else {
            OutcomeSignal::ChecksFailed
        };
        let outcome = OutcomePayload {
            prompt_tx,
            category,
            model: model.to_string(),
            success: signal.success(),
            signal,
            cost_usd,
            subtask_cost_usd: 0.0,
            latency_ms: 60_000,
        };
        self.push(Transaction::outcome(agent, &outcome));
    }

    fn stats(&self) -> GateStats {
        GateStats::from_entries(&self.entries)
    }
}

/// What Gate has recorded: the benchmarks, and Sonnet 5.5's turns
/// outside them, which are 42 debugging turns that passed and the two
/// failed turns on an easy multi-file task (a rename) that had ruled it
/// out of hard multi-file work. Those two were recorded before prompts
/// had a difficulty.
fn history() -> Log {
    let mut log = Log::default();
    for &(_, _, model, kind, cost_usd, passed) in RUNS.iter().chain(LAST) {
        if let Some(work) = kind.work() {
            log.turn(model, work, cost_usd, passed);
        }
    }
    for _ in 0..42 {
        log.turn(SONNET, (PromptCategory::Debugging, None), 0.018, true);
    }
    for _ in 0..2 {
        log.turn(SONNET, (PromptCategory::MultiFileChange, None), 0.01, false);
    }
    log
}

/// What `model` cost on `task` and whether it passed: its latest
/// recorded run.
fn recorded(task: &str, model: &str) -> (f64, bool) {
    let run = LAST
        .iter()
        .rev()
        .chain(RUNS.iter().rev())
        .find(|run| run.1 == task && run.2 == model);
    let Some(run) = run else {
        panic!("no recorded run of {model} on {task}: the replay cannot say how it went");
    };
    (run.4, run.5)
}

/// Gate's routing in a benchmark: no exploring.
fn settings() -> GateConfig {
    GateConfig {
        explore_rate: 0.0,
        ..GateConfig::defaults()
    }
}

/// The benchmark's tasks over again on `log`: each goes where Gate sends
/// hard multi-file work (as their prompts are classified now), costs and
/// ends as the record says that model did on it, and is recorded before
/// the next. A failed attempt goes on with the model Gate moves it up
/// to, and costs both. Returns the models each task was done on, the
/// total cost and how many tasks passed.
fn replay(log: &mut Log, settings: &GateConfig) -> (Vec<Vec<String>>, f64, usize) {
    let hard = Work::turn(PromptCategory::MultiFileChange).of(Some(Difficulty::Hard));
    let candidates = plan();
    let (mut models, mut cost, mut passes) = (Vec::new(), 0.0, 0);
    for task in TASKS {
        let mut next = pick(&log.stats(), &hard, &candidates, settings, &mut || 1.0);
        let mut tried = Vec::new();
        while let Some(decision) = next {
            let model = decision.model.id;
            let (cost_usd, passed) = recorded(task, &model);
            log.turn(
                &model,
                (PromptCategory::MultiFileChange, Some(Difficulty::Hard)),
                cost_usd,
                passed,
            );
            cost += cost_usd;
            next = if passed {
                passes += 1;
                None
            } else {
                // The recordings say how each model did, not at which effort.
                escalate(
                    &log.stats(),
                    RouteLevel::Turn,
                    (&model, None),
                    OutcomeSignal::ChecksFailed,
                    &candidates,
                )
            };
            tried.push(model);
        }
        models.push(tried);
    }
    (models, cost, passes)
}

/// What the strongest model cost on the tasks, and how many it passed.
fn on_opus() -> (f64, usize) {
    TASKS
        .iter()
        .map(|task| recorded(task, OPUS))
        .fold((0.0, 0), |(cost, passes), (cost_usd, passed)| {
            (cost + cost_usd, passes + usize::from(passed))
        })
}

#[test]
fn the_fixture_is_the_recorded_benchmark() {
    let (cost, passes) = on_opus();
    assert!((cost - 1.976).abs() < 1e-9, "{cost}");
    assert_eq!(passes, 3);
    let (theirs, opus) = LAST
        .iter()
        .filter(|run| run.2 == OLD_SONNET)
        .map(|run| (run.4, recorded(run.1, OPUS).0))
        .fold((0.0, 0.0), |sum, run| (sum.0 + run.0, sum.1 + run.1));
    assert!(
        theirs > 1.9 * opus,
        "sonnet 5 cost {theirs} where opus cost {opus}"
    );
    assert!(RUNS.iter().any(|run| run.2 == HAIKU && !run.5));
    let sonnet: Vec<bool> = RUNS
        .iter()
        .filter(|run| run.2 == SONNET)
        .map(|run| run.5)
        .collect();
    assert_eq!(sonnet.len(), 10);
    assert_eq!(sonnet.iter().filter(|passed| **passed).count(), 9);
}

#[test]
fn gate_does_the_recorded_tasks_as_well_as_the_strongest_model_for_less() {
    let mut log = history();
    let (models, cost, passes) = replay(&mut log, &settings());
    let (opus_cost, opus_passes) = on_opus();
    let all = plan();
    for model in models.iter().flatten() {
        assert!(
            current(&all).iter().any(|c| &c.id == model),
            "{model} is an older version of a model Gate can use"
        );
    }
    assert_eq!(
        models,
        [
            vec![SONNET.to_string()],
            vec![SONNET.to_string()],
            vec![SONNET.to_string(), OPUS.to_string()],
        ],
        "hard work starts on the standard model, and what fails goes on to the strongest"
    );
    assert_eq!(passes, opus_passes, "on {models:?}");
    assert!(
        cost < 0.6 * opus_cost,
        "gate spent {cost} on {models:?}, opus {opus_cost}"
    );
}

/// Sonnet 5.5 failing hard multi-file work again and again sends that
/// work to Opus, once starting on Sonnet has come to cost more. Opus has
/// done such work for $0.54 a turn, so at $0.30 a turn Sonnet must pass
/// more than 56% of it: after its one recorded pass, five failures in a
/// row leave it at five in ten at best.
#[test]
fn a_standard_model_that_keeps_failing_the_work_loses_it() {
    let hard = Work::turn(PromptCategory::MultiFileChange).of(Some(Difficulty::Hard));
    let mut log = history();
    let start = |log: &Log| {
        pick(&log.stats(), &hard, &plan(), &settings(), &mut || 1.0)
            .unwrap()
            .model
            .id
    };
    let mut failures = 0;
    while start(&log) == SONNET {
        assert!(failures < 20, "sonnet keeps the work whatever it fails");
        log.turn(
            SONNET,
            (PromptCategory::MultiFileChange, Some(Difficulty::Hard)),
            0.3,
            false,
        );
        failures += 1;
    }
    assert_eq!(start(&log), OPUS);
    assert_eq!(failures, 5);
}
