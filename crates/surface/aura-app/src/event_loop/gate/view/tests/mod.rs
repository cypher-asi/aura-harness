//! `/gate`'s text on what Gate has learned of Claude's models, pinned as
//! the user sees it.
//!
//! - `table`: each level's table, Gate's picks and the models tried
//! - `replay`: the replay of the recorded turns
//! - `delegation`: the share of the active agent's work in subtasks
//! - `recent`: Gate's latest decisions

mod delegation;
mod recent;
mod replay;
mod table;

use super::{money, GateView};
use crate::record_loader::format_local_time;
use aura_config::GateConfig;
use aura_core_types::{PromptCategory, RouteLevel, RouteReason};
use aura_gate::lead::delegation::Delegation;
use aura_gate::live::RecentRoute;
use aura_gate::route::stats::GateStats;
use aura_gate::route::{Candidate, Work};

const HAIKU: &str = "claude/claude-haiku-4-5";
const SONNET: &str = "claude/claude-sonnet-4-6";
const OPUS: &str = "claude/claude-opus-4-7";

/// What each turn the tests' tables learn of cost, in USD.
const TURN_COST_USD: f64 = 0.02;

/// The first line of the view under the default settings.
const SETTINGS: &str = "Gate: a model becomes a category's pick after 10 counted outcomes \
                        at 85% success or better; 5% of routes try the next cheaper model.";

/// The models Gate routes among when only Claude is signed in: Haiku,
/// Sonnet and Opus.
fn claude() -> Vec<Candidate> {
    aura_gate::route::candidates(
        &aura_model::model_catalog::catalog("local/qwen3:4b", &[]),
        |p| p == aura_auth::ProviderKind::Claude,
        |_| false,
    )
}

/// Gate's table, having learned of each of `rows` that so many of the
/// model's turns asking for the category passed, and so many failed.
fn learned(rows: &[(PromptCategory, &str, usize, usize)]) -> GateStats {
    let mut stats = GateStats::default();
    for &(category, model, passed, failed) in rows {
        for outcomes in [(passed, true), (failed, false)] {
            let costs = (TURN_COST_USD, 0.0);
            stats.learn_costing(&Work::turn(category), model, None, outcomes, costs);
        }
    }
    stats
}

/// `/gate` while Gate routes among Claude's models under the default
/// settings, having learned `stats`, with `recent` decisions and
/// `delegation`'s figures.
fn render(stats: &GateStats, recent: &[RecentRoute], delegation: &Delegation) -> String {
    GateView {
        stats,
        candidates: &claude(),
        settings: &GateConfig::defaults(),
        recent,
        pinned: None,
        delegation,
    }
    .render()
}

/// The view most tests read: questions done by Haiku and Sonnet,
/// debugging done by Haiku, and one decision, made at the epoch.
fn learned_view() -> String {
    let stats = learned(&[
        (PromptCategory::Question, HAIKU, 10, 0),
        (PromptCategory::Question, SONNET, 2, 1),
        (PromptCategory::Debugging, HAIKU, 1, 3),
    ]);
    let recent = [RecentRoute {
        ts_ms: 0,
        level: RouteLevel::Turn,
        category: PromptCategory::Question,
        model: "claude-haiku-4-5 (claude plan)".to_string(),
        reason: RouteReason::Proven,
    }];
    render(&stats, &recent, &Delegation::default())
}

/// `line` with each run of spaces in it one space.
fn collapsed(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first line of `text` holding `needle`, [`collapsed`].
fn line(text: &str, needle: &str) -> String {
    let found = text
        .lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?} in:\n{text}"));
    collapsed(found)
}

/// The section of `text` titled `title`, up to the blank line after it.
fn section<'a>(text: &'a str, title: &str) -> &'a str {
    let start = text
        .find(&format!("\n\n{title}"))
        .unwrap_or_else(|| panic!("no {title:?} section in:\n{text}"))
        + 2;
    let rest = &text[start..];
    rest.find("\n\n").map_or(rest, |end| &rest[..end])
}

#[test]
fn every_column_of_the_view_is_pinned() {
    let decision = format!(
        "  {}  turn     question           claude-haiku-4-5 (claude plan)  proven",
        format_local_time(0)
    );
    let expected = [
        SETTINGS,
        "",
        "Turns",
        "  category              model                          tries  success   avg cost",
        "  question            → claude-sonnet-4-6 (claude plan)      3      67%     $0.020  pick (default)",
        "                      → claude-haiku-4-5 (claude plan)      10     100%     $0.020  pick for easy work (proven)",
        "  small_change        → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
        "                      → claude-haiku-4-5 (claude plan)       -        -          -  pick for easy work (default)",
        "  multi_file_change   → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
        "                      → claude-haiku-4-5 (claude plan)       -        -          -  pick for easy work (default)",
        "  debugging           → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
        "                        claude-haiku-4-5 (claude plan)       4      25%     $0.020",
        "  research            → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
        "                      → claude-haiku-4-5 (claude plan)       -        -          -  pick for easy work (default)",
        "  planning            → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
        "  writing             → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
        "                      → claude-haiku-4-5 (claude plan)       -        -          -  pick for easy work (default)",
        "  other               → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
        "                      → claude-haiku-4-5 (claude plan)       -        -          -  pick for easy work (default)",
        "",
        "Replay of 17 recorded turns, each weighted by how unlikely its route was; a policy is judged on the turns it would have routed the same way",
        "  policy                                  matched  success   avg cost",
        "  as recorded                                  17      76%     $0.020",
        "  Gate's picks now                              3      67%     $0.020",
        "  all on claude-haiku-4-5 (claude plan)        14      79%     $0.020",
        "  all on claude-sonnet-4-6 (claude plan)        3      67%     $0.020",
        "",
        "Last 1 decisions, newest last",
        &decision,
    ];
    assert_eq!(learned_view(), expected.join("\n"));
}

#[test]
fn a_pinned_model_and_no_candidates_are_explained() {
    let text = GateView {
        stats: &GateStats::default(),
        candidates: &[],
        settings: &GateConfig::defaults(),
        recent: &[],
        pinned: Some(OPUS),
        delegation: &Delegation::default(),
    }
    .render();
    assert!(
        text.contains("Your turns run on claude/claude-opus-4-7"),
        "{text}"
    );
    assert!(text.contains("Nothing to route to yet"), "{text}");
    assert!(text.contains("No routing decisions yet"), "{text}");
    assert!(!text.contains("Replay"), "nothing recorded to replay");
    let expected = [
        SETTINGS,
        "Your turns run on claude/claude-opus-4-7, which you chose; pick gate in /model to let Gate route them.",
        "Nothing to route to yet: /login, link a plan, or start a local model.",
        "",
        "Turns",
        "  category              model  tries  success   avg cost",
        "",
        "No routing decisions yet in this session.",
    ];
    assert_eq!(
        text,
        expected.join("\n"),
        "with nothing to route to, the view is its headings"
    );
}

#[test]
fn costs_read_as_money() {
    assert_eq!(money(0.0), "free");
    assert_eq!(money(0.0042), "$0.0042");
    assert_eq!(money(0.25), "$0.250");
}
