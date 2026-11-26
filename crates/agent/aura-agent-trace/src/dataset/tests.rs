
use super::*;
use crate::bundle::RunMeta;
use crate::rewards::tests::{base, model, nudge, result};
use aura_core_types::trace::Reward;

fn run(title: &str, outcome: Option<f64>, source: RunSource) -> RunTrace {
    let mut meta = RunMeta::new("m", source);
    meta.title = title.into();
    let steps = vec![
        base(
            1,
            Actor::User,
            StepAction::UserPrompt {
                text: "fix it, key sk-ant-abcdefghijklmnopqrstu".into(),
            },
        ),
        nudge(2, "implement_now"),
        model(3, &[("a", "edit_file", serde_json::json!({"path": "x"}))]),
        result(4, 3, "a", "edit_file", "x", "modify", false),
    ];
    let rewards = outcome
        .map(|v| Reward {
            target: RewardTarget::Run,
            kind: RewardKind::Outcome,
            name: "verify".into(),
            value: v,
            source: "verify".into(),
            at_ms: 1,
            evidence: Value::Null,
        })
        .into_iter()
        .collect();
    RunTrace::from_parts(meta, steps, rewards)
}

#[test]
fn redacts_common_secrets() {
    let (out, n) = redact("token=abcd1234efgh and ghp_abcdefghijklmnopqrstuvwx me@x.io");
    assert_eq!(n, 3);
    assert!(!out.contains("ghp_"));
    assert!(!out.contains("me@x.io"));
}

#[test]
fn splits_are_stable_per_task() {
    assert_eq!(split_for("task-a", 0.5), split_for("task-a", 0.5));
    assert_eq!(split_for("anything", 0.0), "train");
    assert_eq!(split_for("anything", 1.1), "eval");
}

#[test]
fn sft_keeps_successful_non_eval_runs_and_redacts() {
    let dir = tempfile::tempdir().unwrap();
    let traces = vec![
        run("a", Some(1.0), RunSource::Tui),
        run("b", Some(0.0), RunSource::Tui),
        run("c", Some(1.0), RunSource::Eval),
    ];
    let filter = ExportFilter::parse("eval_fraction=0").unwrap();
    let card = export(&traces, None, ExportFormat::Sft, &filter, dir.path()).unwrap();
    assert_eq!(card.runs_used, 1);
    assert_eq!(card.records.get("train"), Some(&1));
    assert!(card.redactions >= 1);
    let text = fs::read_to_string(dir.path().join("train.jsonl")).unwrap();
    assert!(text.contains("[REDACTED]"));
    assert!(dir.path().join(CARD_FILE).is_file());
}

#[test]
fn preference_pairs_same_task_by_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let traces = vec![
        run("same task", Some(1.0), RunSource::Tui),
        run("same task", Some(0.0), RunSource::Tui),
    ];
    let filter = ExportFilter::parse("eval_fraction=0").unwrap();
    let card = export(&traces, None, ExportFormat::Preference, &filter, dir.path()).unwrap();
    assert_eq!(card.records.get("train"), Some(&1));
}

#[test]
fn bandit_rows_carry_effects() {
    let dir = tempfile::tempdir().unwrap();
    let traces = vec![run("a", None, RunSource::Tui)];
    let filter = ExportFilter::parse("eval_fraction=0").unwrap();
    export(
        &traces,
        None,
        ExportFormat::HarnessBandit,
        &filter,
        dir.path(),
    )
    .unwrap();
    let text = fs::read_to_string(dir.path().join("train.jsonl")).unwrap();
    let row: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(row["name"], "implement_now");
    assert_eq!(row["effect"], 1.0);
}

#[test]
fn filter_parses_and_rejects_unknown_keys() {
    let f = ExportFilter::parse("outcome>=0.5,model=x,source=tui").unwrap();
    assert_eq!(f.min_outcome, Some(0.5));
    assert_eq!(f.source, Some(RunSource::Tui));
    assert!(ExportFilter::parse("nope=1").is_err());
}
