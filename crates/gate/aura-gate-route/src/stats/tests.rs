//! Which outcomes the table counts, whatever order entries arrive in,
//! and the rows it answers with.

use aura_core_types::{
    AgentId, Hash, OutcomePayload, OutcomeSignal, PromptCategory, PromptPayload, RecordEntry,
    Route, RouteLevel, RouteReason, Transaction,
};

use super::{GateStats, Row};
use crate::replay::Logged;

const SONNET: &str = "claude/claude-sonnet-4-6";
const HAIKU: &str = "claude/claude-haiku-4-5";

/// Builds one agent's record.
struct Log {
    agent: AgentId,
    entries: Vec<RecordEntry>,
}

impl Log {
    fn new() -> Self {
        Self {
            agent: AgentId::generate(),
            entries: Vec::new(),
        }
    }

    fn push(&mut self, tx: Transaction) -> Hash {
        let hash = tx.hash;
        let seq = u64::try_from(self.entries.len()).unwrap() + 1;
        self.entries.push(RecordEntry::builder(seq, tx).build());
        hash
    }

    fn prompt(&mut self, text: &str, level: RouteLevel) -> Hash {
        self.shaped(text, level, None, None)
    }

    fn shaped(
        &mut self,
        text: &str,
        level: RouteLevel,
        kind: Option<&str>,
        led: Option<bool>,
    ) -> Hash {
        let mut payload = PromptPayload::new(text);
        payload.category = Some(PromptCategory::Debugging);
        payload.route = Route {
            level,
            ..Route::turn(SONNET, RouteReason::Default)
        };
        payload.kind = kind.map(str::to_string);
        payload.led = led;
        self.push(Transaction::user_prompt_with(self.agent, &payload))
    }

    fn outcome(&mut self, prompt_tx: Hash, model: &str, signal: OutcomeSignal) {
        self.outcome_costing(prompt_tx, model, signal, 0.0);
    }

    fn outcome_costing(
        &mut self,
        prompt_tx: Hash,
        model: &str,
        signal: OutcomeSignal,
        subtask_cost_usd: f64,
    ) {
        let outcome = OutcomePayload {
            prompt_tx,
            category: PromptCategory::Debugging,
            model: model.to_string(),
            success: signal.success(),
            signal,
            cost_usd: 0.25,
            subtask_cost_usd,
            latency_ms: 1_000,
        };
        self.push(Transaction::outcome(self.agent, &outcome));
    }
}

fn debugging(stats: &GateStats, level: RouteLevel, model: &str) -> Option<(u32, u32)> {
    stats
        .row(level, PromptCategory::Debugging, model)
        .map(|row| (row.tries, row.successes))
}

#[test]
fn each_prompt_counts_once_with_its_latest_known_outcome() {
    let mut log = Log::new();
    let first = log.prompt("fix the parser", RouteLevel::Turn);
    log.outcome(first, SONNET, OutcomeSignal::ChecksPassed);
    log.outcome(first, SONNET, OutcomeSignal::Rewind);
    let second = log.prompt("fix the lexer", RouteLevel::Turn);
    log.outcome(second, SONNET, OutcomeSignal::Cancelled);
    let third = log.prompt("fix the build", RouteLevel::Turn);
    log.outcome(third, SONNET, OutcomeSignal::ChecksPassed);

    let stats = GateStats::from_entries(&log.entries);
    assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((2, 1)));
    let row = stats
        .row(RouteLevel::Turn, PromptCategory::Debugging, SONNET)
        .unwrap();
    assert_eq!(row.success_rate(), Some(0.5));
    assert_eq!(row.mean_cost_usd(), Some(0.25));
    assert_eq!(
        (row.turns, row.latency_ms),
        (3, 3_000),
        "the cancelled turn still took its time"
    );
}

#[test]
fn a_cancelled_turn_counts_for_its_cost_only() {
    let mut log = Log::new();
    let checked = log.prompt("fix the parser", RouteLevel::Turn);
    log.outcome(checked, SONNET, OutcomeSignal::ChecksPassed);
    let unchecked = log.prompt("fix the lexer", RouteLevel::Turn);
    log.outcome_costing(unchecked, SONNET, OutcomeSignal::Cancelled, 0.75);
    for stats in [
        GateStats::from_entries(&log.entries),
        GateStats::from_entries(log.entries.iter().rev()),
    ] {
        let row = *stats
            .row(RouteLevel::Turn, PromptCategory::Debugging, SONNET)
            .unwrap();
        assert_eq!((row.turns, row.tries, row.successes), (2, 1, 1));
        assert_eq!(row.success_rate(), Some(1.0), "it says nothing of success");
        assert_eq!(
            row.mean_total_cost_usd(),
            Some(0.625),
            "(0.25 + 0.25 + 0.75) over both turns"
        );
        assert!(!row.settled(2));
        assert_eq!(
            stats.mean_cost_usd(RouteLevel::Turn, SONNET),
            Some(0.625),
            "and in what the model costs overall"
        );
    }

    // A verdict for the same turn takes its place, in whichever order
    // the two are seen: the turn is not counted twice, and an
    // outcome that says nothing never replaces one that does.
    log.outcome(unchecked, SONNET, OutcomeSignal::ChecksFailed);
    log.outcome(checked, SONNET, OutcomeSignal::Cancelled);
    for stats in [
        GateStats::from_entries(&log.entries),
        GateStats::from_entries(log.entries.iter().rev()),
    ] {
        let row = *stats
            .row(RouteLevel::Turn, PromptCategory::Debugging, SONNET)
            .unwrap();
        assert_eq!((row.turns, row.tries, row.successes), (2, 2, 1));
        assert_eq!(row.mean_total_cost_usd(), Some(0.25));
    }

    // Alone on a model, it leaves a row with a cost and no tries.
    let mut log = Log::new();
    let only = log.prompt("fix the build", RouteLevel::Turn);
    log.outcome(only, SONNET, OutcomeSignal::Cancelled);
    let stats = GateStats::from_entries(&log.entries);
    assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((0, 0)));
    assert!(stats.logged().is_empty(), "replay keeps to known results");
}

#[test]
fn superseding_outcomes_replace_rather_than_add() {
    let mut log = Log::new();
    let prompt = log.prompt("fix it", RouteLevel::Turn);
    log.outcome(prompt, SONNET, OutcomeSignal::ChecksPassed);
    let mut stats = GateStats::default();
    for entry in &log.entries {
        stats.observe(entry);
    }
    assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((1, 1)));

    log.outcome(prompt, SONNET, OutcomeSignal::JudgedFail);
    stats.observe(log.entries.last().unwrap());
    assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((1, 0)));

    log.outcome(prompt, SONNET, OutcomeSignal::JudgedPass);
    stats.observe(log.entries.last().unwrap());
    stats.observe(log.entries.last().unwrap());
    assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((1, 1)));
    assert_eq!(stats.rows().len(), 1);
}

#[test]
fn a_replacing_outcome_on_another_model_moves_the_count() {
    let mut log = Log::new();
    let prompt = log.prompt("fix it", RouteLevel::Turn);
    log.outcome(prompt, HAIKU, OutcomeSignal::ChecksPassed);
    log.outcome(prompt, SONNET, OutcomeSignal::ChecksFailed);
    let stats = GateStats::from_entries(&log.entries);
    assert_eq!(debugging(&stats, RouteLevel::Turn, HAIKU), None);
    assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((1, 0)));
}

#[test]
fn order_and_repeats_do_not_change_the_table() {
    let mut log = Log::new();
    for text in ["fix a", "fix b", "fix c"] {
        let prompt = log.prompt(text, RouteLevel::Turn);
        log.outcome(prompt, SONNET, OutcomeSignal::ChecksPassed);
        log.outcome(prompt, SONNET, OutcomeSignal::Rewind);
        log.outcome(prompt, SONNET, OutcomeSignal::JudgedPass);
    }
    let in_order = GateStats::from_entries(&log.entries);
    let reversed = GateStats::from_entries(log.entries.iter().rev());
    let twice = GateStats::from_entries(log.entries.iter().chain(&log.entries));
    for stats in [&reversed, &twice] {
        assert_eq!(stats.rows(), in_order.rows());
    }
    assert_eq!(debugging(&in_order, RouteLevel::Turn, SONNET), Some((3, 3)));
}

#[test]
fn the_same_prompt_in_two_agents_counts_twice() {
    let mut first = Log::new();
    let mut second = Log::new();
    let a = first.prompt("fix it", RouteLevel::Turn);
    let b = second.prompt("fix it", RouteLevel::Turn);
    assert_eq!(a, b, "the hash covers only the payload");
    first.outcome(a, SONNET, OutcomeSignal::ChecksPassed);
    second.outcome(b, SONNET, OutcomeSignal::ChecksFailed);
    let stats = GateStats::from_entries(first.entries.iter().chain(&second.entries));
    assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((2, 1)));
}

#[test]
fn an_escalated_retry_is_recorded_but_not_counted() {
    let mut log = Log::new();
    let first = log.prompt("fix that part", RouteLevel::Subtask);
    log.outcome(first, SONNET, OutcomeSignal::ChecksFailed);
    let mut retry = PromptPayload::new("fix that part, again");
    retry.category = Some(PromptCategory::Debugging);
    retry.route = Route::subtask(HAIKU, RouteReason::Escalate);
    let retry = log.push(Transaction::user_prompt_with(log.agent, &retry));
    log.outcome(retry, HAIKU, OutcomeSignal::ChecksPassed);

    for stats in [
        GateStats::from_entries(&log.entries),
        GateStats::from_entries(log.entries.iter().rev()),
    ] {
        assert_eq!(debugging(&stats, RouteLevel::Subtask, SONNET), Some((1, 0)));
        assert_eq!(debugging(&stats, RouteLevel::Subtask, HAIKU), None);
        assert_eq!(stats.rows().len(), 1);
    }
}

#[test]
fn a_models_mean_cost_counts_what_its_subtasks_cost() {
    let mut log = Log::new();
    let led = log.shaped("fix it", RouteLevel::Turn, None, Some(true));
    log.outcome_costing(led, SONNET, OutcomeSignal::ChecksPassed, 0.75);
    let stats = GateStats::from_entries(&log.entries);
    assert_eq!(stats.mean_cost_usd(RouteLevel::Turn, SONNET), Some(1.0));
}

#[test]
fn the_replay_log_holds_each_counted_prompt_with_its_propensity() {
    let mut log = Log::new();
    let mut explored = PromptPayload::new("fix the parser");
    explored.category = Some(PromptCategory::Debugging);
    explored.route = Route {
        explored: true,
        propensity: 0.05,
        ..Route::turn(HAIKU, RouteReason::Explore)
    };
    let explored = log.push(Transaction::user_prompt_with(log.agent, &explored));
    log.outcome_costing(explored, HAIKU, OutcomeSignal::ChecksPassed, 0.5);
    let unknown = log.prompt("fix the lexer", RouteLevel::Turn);
    log.outcome(unknown, SONNET, OutcomeSignal::Cancelled);
    let mut retry = PromptPayload::new("fix the parser, again");
    retry.route = Route::turn(SONNET, RouteReason::Escalate);
    let retry = log.push(Transaction::user_prompt_with(log.agent, &retry));
    log.outcome(retry, SONNET, OutcomeSignal::ChecksPassed);

    let logged = GateStats::from_entries(&log.entries).logged();
    assert_eq!(
        logged,
        [Logged {
            level: RouteLevel::Turn,
            category: PromptCategory::Debugging,
            difficulty: None,
            kind: None,
            model: HAIKU.to_string(),
            propensity: 0.05,
            success: true,
            cost_usd: 0.75,
        }]
    );
}

#[test]
fn a_failure_the_judge_excuses_counts_for_nothing() {
    let mut log = Log::new();
    let excused = log.prompt("fix the build", RouteLevel::Turn);
    log.outcome(excused, SONNET, OutcomeSignal::ChecksFailed);
    log.outcome(excused, SONNET, OutcomeSignal::JudgedExcused);
    let confirmed = log.prompt("fix the parser", RouteLevel::Turn);
    log.outcome(confirmed, SONNET, OutcomeSignal::ChecksFailed);
    log.outcome(confirmed, SONNET, OutcomeSignal::JudgedFail);
    for stats in [
        GateStats::from_entries(&log.entries),
        GateStats::from_entries(log.entries.iter().rev()),
    ] {
        assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((1, 0)));
    }

    // A later signal still has the last word.
    log.outcome(excused, SONNET, OutcomeSignal::ChecksPassed);
    let stats = GateStats::from_entries(&log.entries);
    assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((2, 1)));

    let mut log = Log::new();
    let only = log.prompt("fix the build", RouteLevel::Turn);
    log.outcome(only, SONNET, OutcomeSignal::ChecksFailed);
    log.outcome(only, SONNET, OutcomeSignal::JudgedExcused);
    let stats = GateStats::from_entries(&log.entries);
    assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), None);
    assert!(stats.rows().is_empty(), "no row is left with nothing in it");
}

#[test]
fn a_model_is_ruled_out_once_it_cannot_reach_the_target() {
    let row = |tries, successes| Row {
        tries,
        successes,
        ..Row::default()
    };
    assert!(row(0, 0).can_still_reach(0.85, 10));
    assert!(row(1, 0).can_still_reach(0.85, 10), "9 of 10 would do");
    assert!(!row(2, 0).can_still_reach(0.85, 10), "8 of 10 would not");
    assert!(row(9, 8).can_still_reach(0.85, 10));
    assert!(!row(9, 7).can_still_reach(0.85, 10));
    assert!(row(20, 17).can_still_reach(0.85, 10));
    assert!(!row(20, 16).can_still_reach(0.85, 10));
}

#[test]
fn levels_learn_separately() {
    let mut log = Log::new();
    let turn = log.prompt("fix it", RouteLevel::Turn);
    log.outcome(turn, SONNET, OutcomeSignal::ChecksPassed);
    let subtask = log.prompt("fix that part", RouteLevel::Subtask);
    log.outcome(subtask, SONNET, OutcomeSignal::ChecksFailed);
    let stats = GateStats::from_entries(&log.entries);
    assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((1, 1)));
    assert_eq!(debugging(&stats, RouteLevel::Subtask, SONNET), Some((1, 0)));
    assert_eq!(stats.mean_cost_usd(RouteLevel::Turn, SONNET), Some(0.25));
    assert_eq!(stats.mean_cost_usd(RouteLevel::Turn, HAIKU), None);
}

#[test]
fn subtask_kinds_learn_apart_from_the_level_row() {
    let mut log = Log::new();
    let rust = log.shaped(
        "fix the borrow",
        RouteLevel::Subtask,
        Some("rust-engineer"),
        None,
    );
    log.outcome(rust, SONNET, OutcomeSignal::ChecksPassed);
    let tests = log.shaped("cover the parser", RouteLevel::Subtask, Some("tests"), None);
    log.outcome(tests, SONNET, OutcomeSignal::ChecksFailed);
    let turn = log.shaped("fix it", RouteLevel::Turn, Some("rust-engineer"), None);
    log.outcome(turn, SONNET, OutcomeSignal::ChecksPassed);

    for stats in [
        GateStats::from_entries(&log.entries),
        GateStats::from_entries(log.entries.iter().rev()),
    ] {
        assert_eq!(debugging(&stats, RouteLevel::Subtask, SONNET), Some((2, 1)));
        let kind = |kind| {
            stats
                .kind_row(PromptCategory::Debugging, kind, SONNET)
                .map(|row| (row.tries, row.successes))
        };
        assert_eq!(
            kind("rust-engineer"),
            Some((1, 1)),
            "the turn's kind is not a subtask's"
        );
        assert_eq!(kind("tests"), Some((1, 0)));
        assert_eq!(kind("explorer"), None);
    }
}

#[test]
fn led_and_direct_turns_learn_apart_with_subtask_costs() {
    let mut log = Log::new();
    let led = log.shaped("move the module", RouteLevel::Turn, None, Some(true));
    log.outcome_costing(led, SONNET, OutcomeSignal::ChecksPassed, 1.0);
    let direct = log.shaped("move the other", RouteLevel::Turn, None, Some(false));
    log.outcome(direct, SONNET, OutcomeSignal::ChecksFailed);
    let unsaid = log.prompt("move a third", RouteLevel::Turn);
    log.outcome(unsaid, SONNET, OutcomeSignal::ChecksPassed);
    let mut stats = GateStats::from_entries(&log.entries);

    assert_eq!(debugging(&stats, RouteLevel::Turn, SONNET), Some((3, 2)));
    let led_row = stats
        .way_row(PromptCategory::Debugging, SONNET, true)
        .unwrap();
    assert_eq!((led_row.tries, led_row.successes), (1, 1));
    assert_eq!(led_row.mean_total_cost_usd(), Some(1.25));
    let direct_row = stats
        .way_row(PromptCategory::Debugging, SONNET, false)
        .unwrap();
    assert_eq!((direct_row.tries, direct_row.successes), (1, 0));
    assert_eq!(direct_row.mean_total_cost_usd(), Some(0.25));

    log.outcome(led, SONNET, OutcomeSignal::JudgedFail);
    stats.observe(log.entries.last().unwrap());
    let led_row = stats
        .way_row(PromptCategory::Debugging, SONNET, true)
        .unwrap();
    assert_eq!(
        (led_row.tries, led_row.successes),
        (1, 0),
        "replaced, not added"
    );
    assert_eq!(led_row.mean_total_cost_usd(), Some(0.25));
    let turns = |row: Option<&Row>| row.map(|row| row.turns);
    assert_eq!(
        [
            turns(stats.row(RouteLevel::Turn, PromptCategory::Debugging, SONNET)),
            turns(stats.way_row(PromptCategory::Debugging, SONNET, true)),
            turns(stats.way_row(PromptCategory::Debugging, SONNET, false)),
        ],
        [Some(3), Some(1), Some(1)],
        "the unsaid turn is in neither way's row"
    );
}
