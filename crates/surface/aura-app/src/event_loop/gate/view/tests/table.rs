//! Each level's table: Gate's picks for each category, then the models
//! tried there.

use super::{collapsed, learned_view, line, render, section, HAIKU, OPUS, SONNET};
use aura_core_types::PromptCategory::{Debugging, Question, Research};
use aura_core_types::{
    AgentId, OutcomePayload, OutcomeSignal, PromptCategory, PromptPayload, RecordEntry, Route,
    RouteLevel, RouteReason, Transaction,
};
use aura_gate::lead::delegation::Delegation;
use aura_gate::route::stats::GateStats;
use aura_gate::route::Work;

#[test]
fn each_category_lists_gates_picks_then_the_models_tried() {
    let text = learned_view();
    assert!(text.contains("after 10 counted outcomes at 85%"), "{text}");
    assert_eq!(
        line(&text, "question"),
        "question → claude-sonnet-4-6 (claude plan) 3 67% $0.020 pick (default)",
        "work that is not easy starts no lower than sonnet, proven haiku or not"
    );
    assert_eq!(
        line(&text, "pick for easy work (proven)"),
        "→ claude-haiku-4-5 (claude plan) 10 100% $0.020 pick for easy work (proven)"
    );
    assert_eq!(
        line(&text, "debugging"),
        "debugging → claude-sonnet-4-6 (claude plan) - - - pick (default)"
    );
    let lines: Vec<String> = text.lines().map(collapsed).collect();
    assert!(
        lines.contains(&"claude-haiku-4-5 (claude plan) 4 25% $0.020".to_string()),
        "haiku is out of reach on debugging, so easy debugging goes to sonnet too:\n{text}"
    );
    assert!(line(&text, "writing").ends_with("pick (default)"));
    assert!(
        lines.contains(
            &"→ claude-haiku-4-5 (claude plan) - - - pick for easy work (default)".to_string()
        ),
        "{text}"
    );
    assert!(!text.contains("Subtasks"), "no subtask outcomes yet");
}

/// Take in a subtask asking for `category` on `model` that the user
/// cancelled, which counts for what it cost and not as a try. The route
/// crate's builder only learns outcomes that passed or failed.
fn cancelled(stats: &mut GateStats, category: PromptCategory, model: &str) {
    let agent = AgentId::generate();
    let mut prompt = PromptPayload::new("cancelled");
    prompt.category = Some(category);
    prompt.route = Route::subtask(model, RouteReason::Default);
    let prompt = Transaction::user_prompt_with(agent, &prompt);
    let outcome = OutcomePayload {
        prompt_tx: prompt.hash,
        category,
        model: model.to_string(),
        success: None,
        signal: OutcomeSignal::Cancelled,
        cost_usd: 0.03,
        subtask_cost_usd: 0.0,
        latency_ms: 10,
    };
    stats.observe(&RecordEntry::builder(1, prompt).build());
    stats.observe(&RecordEntry::builder(2, Transaction::outcome(agent, &outcome)).build());
}

/// Gate's table, having learned subtasks of Claude's models, of two
/// models that are not candidates, and of Opus's research, cancelled.
fn subtasks_learned() -> GateStats {
    let mut stats = GateStats::default();
    let rows = [
        (Question, HAIKU, (3, true), 0.02),
        (Debugging, SONNET, (1, true), 0.004),
        (Debugging, SONNET, (2, false), 0.004),
        (Debugging, "openai/gpt-5.4", (1, true), 0.05),
        (Debugging, OPUS, (2, true), 0.1),
        (Debugging, "local/qwen3:4b", (1, false), 0.0),
    ];
    for (category, model, outcomes, cost_usd) in rows {
        let work = Work::new(RouteLevel::Subtask, category, None);
        stats.learn_costing(&work, model, None, outcomes, (cost_usd, 0.0));
    }
    cancelled(&mut stats, Research, OPUS);
    stats
}

#[test]
fn the_subtask_table_lists_the_categories_with_outcomes_column_for_column() {
    let expected = [
        "Subtasks",
        "  category              model                          tries  success   avg cost",
        "  question            → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
        "                      → claude-haiku-4-5 (claude plan)       3     100%     $0.020  pick for easy work (default)",
        "  debugging           → claude-sonnet-4-6 (claude plan)      3      33%    $0.0040  pick (default)",
        "                      → claude-haiku-4-5 (claude plan)       -        -          -  pick for easy work (default)",
        "                        claude-opus-4-7 (claude plan)        2     100%     $0.100",
        "                        qwen3:4b (local)                     1       0%       free",
        "                        openai/gpt-5.4 (cloud)               1     100%     $0.050",
        "  research            → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
        "                      → claude-haiku-4-5 (claude plan)       -        -          -  pick for easy work (default)",
        "                        claude-opus-4-7 (claude plan)        0              $0.030",
    ];
    let text = render(&subtasks_learned(), &[], &Delegation::default());
    assert_eq!(section(&text, "Subtasks"), expected.join("\n"));
}
