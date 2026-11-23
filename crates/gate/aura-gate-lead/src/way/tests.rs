//! [`choose_way`] on what Gate has learned of turns done both ways.

use super::{choose_way, Way};
use aura_config::GateConfig;
use aura_core_types::{
    AgentId, Difficulty, OutcomePayload, OutcomeSignal, PromptCategory, PromptPayload, RecordEntry,
    Route, RouteReason, Transaction,
};
use aura_gate_route::stats::GateStats;

const SONNET: &str = "claude/claude-sonnet-4-6";
const OPUS: &str = "claude/claude-opus-4-7";

/// A roll that keeps the best known way.
const KEPT: f64 = 0.99;
/// A roll that takes the other way, where exploring may.
const EXPLORED: f64 = 0.0;

/// `n` turns on Sonnet asking for `category`, led or not, that
/// all passed or all failed, each costing `cost_usd` in the lead
/// and `subtask_cost_usd` in subtasks.
fn turns(
    entries: &mut Vec<RecordEntry>,
    n: usize,
    category: PromptCategory,
    led: bool,
    ok: bool,
    costs: (f64, f64),
) {
    turns_of(entries, n, (category, None), led, ok, costs);
}

/// [`turns`] the classifier judged of `difficulty`.
fn turns_of(
    entries: &mut Vec<RecordEntry>,
    n: usize,
    (category, difficulty): (PromptCategory, Option<Difficulty>),
    led: bool,
    ok: bool,
    (cost_usd, subtask_cost_usd): (f64, f64),
) {
    let signal = if ok {
        OutcomeSignal::ChecksPassed
    } else {
        OutcomeSignal::ChecksFailed
    };
    for _ in 0..n {
        let agent = AgentId::generate();
        let mut prompt = PromptPayload::new(format!("turn {}", entries.len()));
        prompt.category = Some(category);
        prompt.route = Route::turn(SONNET, RouteReason::Default);
        prompt.led = Some(led);
        prompt.difficulty = difficulty;
        let prompt = Transaction::user_prompt_with(agent, &prompt);
        let outcome = OutcomePayload {
            prompt_tx: prompt.hash,
            category,
            model: SONNET.to_string(),
            success: signal.success(),
            signal,
            cost_usd,
            subtask_cost_usd,
            latency_ms: 10,
        };
        for tx in [prompt, Transaction::outcome(agent, &outcome)] {
            let seq = u64::try_from(entries.len()).unwrap() + 1;
            entries.push(RecordEntry::builder(seq, tx).build());
        }
    }
}

fn settings(explore_rate: f64) -> GateConfig {
    GateConfig {
        explore_rate,
        ..GateConfig::defaults()
    }
}

fn way(entries: &[RecordEntry], category: PromptCategory, roll: f64) -> Way {
    let stats = GateStats::from_entries(entries);
    choose_way(
        &stats,
        (category, None),
        SONNET,
        &settings(0.05),
        &mut || roll,
    )
}

#[test]
fn with_nothing_learned_the_model_does_the_work() {
    for category in PromptCategory::CLASSES {
        assert!(!way(&[], category, KEPT).led, "{category}");
    }
    let tried = way(&[], PromptCategory::MultiFileChange, EXPLORED);
    assert_eq!((tried.led, tried.explored), (true, true));
    let never = way(&[], PromptCategory::Question, EXPLORED);
    assert_eq!((never.led, never.explored), (false, false));
}

#[test]
fn a_frontier_model_leads_only_once_leading_is_proven_cheaper() {
    let category = PromptCategory::Planning;
    let on_opus = |stats: &GateStats, roll: f64| {
        choose_way(stats, (category, None), OPUS, &settings(0.05), &mut || roll)
    };
    let empty = GateStats::default();
    let kept = on_opus(&empty, EXPLORED);
    assert_eq!((kept.led, kept.explored), (false, false), "never explored");
    assert!(!on_opus(&empty, KEPT).led);
}

#[test]
fn of_two_proven_ways_the_cheaper_in_total_wins() {
    let category = PromptCategory::MultiFileChange;
    let mut entries = Vec::new();
    turns(&mut entries, 10, category, true, true, (0.10, 0.50));
    turns(&mut entries, 10, category, false, true, (0.40, 0.0));
    assert!(!way(&entries, category, KEPT).led, "0.40 beats 0.60");

    let mut entries = Vec::new();
    turns(&mut entries, 10, category, true, true, (0.10, 0.20));
    turns(&mut entries, 10, category, false, true, (0.40, 0.0));
    assert!(way(&entries, category, KEPT).led, "0.30 beats 0.40");
}

#[test]
fn proven_leading_waits_until_working_directly_is_proven_or_ruled_out() {
    let category = PromptCategory::Debugging;
    let mut entries = Vec::new();
    turns(&mut entries, 10, category, true, true, (0.1, 0.1));
    assert!(
        !way(&entries, category, KEPT).led,
        "only the led way is proven: working directly gets its tries"
    );
    turns(&mut entries, 9, category, false, true, (0.4, 0.0));
    assert!(!way(&entries, category, KEPT).led, "not proven yet");
    turns(&mut entries, 1, category, false, true, (0.4, 0.0));
    assert!(way(&entries, category, KEPT).led, "0.20 beats 0.40");

    let mut entries = Vec::new();
    turns(&mut entries, 10, category, true, true, (1.0, 1.0));
    turns(&mut entries, 3, category, false, false, (0.1, 0.0));
    assert!(
        way(&entries, category, KEPT).led,
        "working directly is ruled out"
    );

    let category = PromptCategory::MultiFileChange;
    let mut entries = Vec::new();
    turns(&mut entries, 3, category, false, false, (0.1, 0.0));
    assert!(
        way(&entries, category, KEPT).led,
        "3 failures rule working directly out"
    );
    turns(&mut entries, 3, category, true, false, (0.1, 0.1));
    assert!(
        !way(&entries, category, KEPT).led,
        "both ruled out: the model does the work"
    );
}

#[test]
fn leading_proven_cheaper_on_easy_work_says_nothing_of_hard_work() {
    let category = PromptCategory::MultiFileChange;
    let (easy, hard) = (Some(Difficulty::Easy), Some(Difficulty::Hard));
    let mut entries = Vec::new();
    turns_of(&mut entries, 10, (category, easy), true, true, (0.06, 0.03));
    turns_of(&mut entries, 10, (category, hard), false, true, (0.50, 0.0));
    let stats = GateStats::from_entries(&entries);
    let way = |difficulty| {
        choose_way(
            &stats,
            (category, difficulty),
            SONNET,
            &settings(0.0),
            &mut || KEPT,
        )
        .led
    };
    assert!(!way(hard), "nothing proves leading hard work cheaper");
    assert!(way(None), "over every turn, leading looks cheaper");

    turns_of(&mut entries, 10, (category, hard), true, true, (0.30, 0.40));
    let stats = GateStats::from_entries(&entries);
    assert!(
        !choose_way(
            &stats,
            (category, hard),
            SONNET,
            &settings(0.0),
            &mut || KEPT
        )
        .led,
        "led hard work costs 0.70 in all, done directly 0.50"
    );
}

#[test]
fn exploring_never_takes_a_ruled_out_way() {
    let category = PromptCategory::Research;
    let mut entries = Vec::new();
    turns(&mut entries, 3, category, true, false, (0.1, 0.1));
    let way = way(&entries, category, EXPLORED);
    assert_eq!((way.led, way.explored), (false, false));
}
