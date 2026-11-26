use super::*;
use crate::bundle::RunSource;
use crate::rewards::tests::{model, nudge, result};
use serde_json::json;

#[test]
fn counts_waste_and_first_edit() {
    let steps = vec![
        model(1, &[("a", "read_file", json!({"path": "x"}))]),
        result(2, 1, "a", "read_file", "x", "read", false),
        nudge(3, "implement_now"),
        model(4, &[("b", "edit_file", json!({"path": "x"}))]),
        result(5, 4, "b", "edit_file", "x", "modify", true),
        model(6, &[("c", "edit_file", json!({"path": "x"}))]),
        result(7, 6, "c", "edit_file", "x", "modify", false),
    ];
    let rewards = crate::rewards::derive(&steps);
    let meta = RunMeta::new("m", RunSource::Test);
    let card = RunScorecard::compute(&meta, &steps, &rewards);
    assert_eq!(card.model_calls, 3);
    assert_eq!(card.failed_edits, 1);
    assert_eq!(card.writes, 1);
    assert_eq!(card.ms_to_first_edit, Some(6000));
    assert_eq!(card.tokens_to_first_edit, Some(3300));
    assert_eq!(card.decisions.get("steering/implement_now"), Some(&1));
    assert_eq!(card.effects["implement_now"].rate(), Some(1.0));
}

#[test]
fn prices_each_call_by_its_model() {
    let mut opus = model(1, &[]);
    opus.policy.id = "claude-opus-5-5".into();
    let mut haiku = model(2, &[]);
    haiku.policy.id = "claude-haiku-5-5".into();
    let mut local = model(3, &[]);
    local.policy.id = "local/qwen3:4b".into();
    let steps = vec![opus, haiku, local];
    let card = RunScorecard::compute(&RunMeta::new("m", RunSource::Test), &steps, &[]);
    // 1000 in + 100 out: opus $0.006, haiku $0.00015.
    assert!((card.cost_usd - 0.006_15).abs() < 1e-9, "{}", card.cost_usd);
    assert_eq!(card.unpriced_calls, 1);
    assert!(mean(&[card])["cost_usd"] > 0.006);
}

#[test]
fn diff_flags_regressions_past_tolerance() {
    let base = RunScorecard {
        model_calls: 10,
        outcome: Some(1.0),
        ..RunScorecard::default()
    };
    let cand = RunScorecard {
        model_calls: 12,
        outcome: Some(1.0),
        ..RunScorecard::default()
    };
    let d = diff(&base, &cand, 0.1);
    assert_eq!(d.len(), 1);
    assert!(d[0].regression);
    assert!(!diff(&base, &cand, 0.5)[0].regression);
}
