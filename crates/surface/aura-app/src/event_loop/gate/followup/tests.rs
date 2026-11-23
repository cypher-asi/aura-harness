//! Which steps an attempt may take, what it is told of them, and what the
//! harness's own check and the review say.

use super::check::check_signal;
use super::messages::{nudge_message, nudge_notice};
use super::review::Reviewed;
use super::untested::untested;
use super::*;
use crate::event_loop::task_check::CheckRun;
use aura_config::GateConfig;
use aura_core_types::{Hash, OutcomePayload};
use aura_gate::followup::{Allowed, NUDGE_PROMPT};
use aura_gate::judge::criteria::{Checked, Criterion, Kind, Status};
use aura_gate::judge::turn::Answer;
use aura_model::model_catalog::{ModelBackend, ModelTier};
use aura_model::ThinkingEffort;

const HAIKU: &str = "claude/claude-haiku-5-5";
const SONNET: &str = "claude/claude-sonnet-5-5";
const OPUS: &str = "claude/claude-opus-5-5";

fn candidate(id: &str, tier: ModelTier, cost_usd: f64) -> Candidate {
    Candidate {
        id: id.to_string(),
        label: id.to_string(),
        backend: ModelBackend::Claude,
        tier,
        cost_usd: Some(cost_usd),
    }
}

fn never_exploring() -> GateLive {
    GateLive::new(GateConfig::defaults().without_exploring())
}

#[test]
fn a_review_held_to_criteria_says_how_many_it_found_met() {
    let checked = |status| Checked {
        criterion: Criterion {
            text: "the body is byte-identical to MIMEMultipart's".to_string(),
            kind: Kind::ReferenceMatch,
            reference: None,
        },
        status,
    };
    let reviewed = Reviewed {
        model: "claude/claude-haiku-4-5".to_string(),
        judge: OPUS.to_string(),
        answer: Answer::Model,
        findings: "CRITERION 2: UNTESTED".to_string(),
        cost_usd: 0.1,
        act: false,
        checked: vec![
            checked(Some(Status::Met)),
            checked(Some(Status::Untested)),
            checked(None),
        ],
    };
    let shown = reviewed.describe();
    assert!(
        shown.contains(", criteria 1/3 met, 1 untested): "),
        "{shown}"
    );
    assert!(
        shown.contains("AURA_GATE_CRITERIA=on acts on it"),
        "{shown}"
    );
    let plain = Reviewed {
        checked: Vec::new(),
        ..reviewed
    };
    let shown = plain.describe();
    assert!(!shown.contains("criteria"), "{shown}");
    assert!(shown.contains("AURA_GATE_REVIEW=on acts on it"), "{shown}");
}

#[tokio::test]
async fn what_no_test_names_is_untested() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let write = |path: &str, text: &str| {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("lib/urls.py", "def prepare_multipart(fields):\n    pass\n");
    write("test/units/test_urls.py", "from urls import open_url\n");
    write(".aura/tasks/1.json", "implement prepare_multipart");
    write("node_modules/x/test.js", "prepare_multipart()");
    let names = vec!["prepare_multipart".to_string()];

    let found = untested(root.to_path_buf(), names.clone()).await;
    assert_eq!(
        found, names,
        "the prompt's records and dependencies are not tests"
    );

    write(
        "test/units/test_multipart.py",
        "from urls import prepare_multipart\n",
    );
    assert!(untested(root.to_path_buf(), names).await.is_empty());
}

#[test]
fn work_with_no_stronger_model_to_go_to_gets_only_the_judge() {
    const SONNET_4: &str = "claude/claude-sonnet-4-6";
    const OPUS_4: &str = "claude/claude-opus-4-7";
    let candidates = [
        candidate(SONNET_4, ModelTier::Standard, 3.0),
        candidate(OPUS_4, ModelTier::Frontier, 15.0),
    ];
    let live = never_exploring();
    let workspace = tempfile::tempdir().unwrap();
    let at_high = |model| {
        FollowUp::new(
            &live,
            &candidates,
            (model, Some(ThinkingEffort::High)),
            workspace.path(),
        )
    };
    assert!(at_high(SONNET_4).allowed.nudge);
    let top = at_high(OPUS_4);
    assert_eq!(
        top.allowed,
        Allowed {
            judge: top.judge.is_some(),
            review: ReviewMode::Off,
            criteria: ReviewMode::Off,
            ..Allowed::default()
        },
        "max on the same model is not somewhere to go"
    );
}

#[test]
fn work_is_judged_and_reviewed_on_its_own_model_from_the_standard_tier_up() {
    let candidates = [
        candidate(HAIKU, ModelTier::Fast, 0.1),
        candidate(SONNET, ModelTier::Standard, 2.0),
        candidate(OPUS, ModelTier::Frontier, 4.0),
    ];
    let live = never_exploring();
    let workspace = tempfile::tempdir().unwrap();
    let closing = |model| {
        let follow_up = FollowUp::new(&live, &candidates, (model, None), workspace.path());
        (follow_up.judge, follow_up.reviewer)
    };
    let sonnet = (Some(SONNET.to_string()), Some(SONNET.to_string()));
    assert_eq!(closing(HAIKU), sonnet, "a fast model's work goes to Sonnet");
    assert_eq!(closing(SONNET), sonnet);
    assert_eq!(
        closing(OPUS),
        (Some(OPUS.to_string()), None),
        "the strongest tier's work is judged but not reviewed"
    );

    let failed = OutcomePayload {
        prompt_tx: Hash::from_content(b"fix the parser"),
        category: PromptCategory::Debugging,
        model: SONNET.to_string(),
        success: Some(false),
        signal: OutcomeSignal::ChecksFailed,
        cost_usd: 0.01,
        subtask_cost_usd: 0.0,
        latency_ms: 10,
    };
    assert_eq!(
        live::judge_for(&live, &candidates, &failed).as_deref(),
        Some(OPUS),
        "whether a failure was the model's fault is still the strongest model's call"
    );
}

#[test]
fn a_review_that_is_only_logged_is_asked_on_a_sample_of_attempts() {
    let candidates = [
        candidate(SONNET, ModelTier::Standard, 2.0),
        candidate(OPUS, ModelTier::Frontier, 4.0),
    ];
    let workspace = tempfile::tempdir().unwrap();
    let gate = |review, criteria, review_sample| {
        let settings = GateConfig {
            review,
            criteria,
            review_sample,
            ..GateConfig::defaults().without_exploring()
        };
        GateLive::with_rng(settings, aura_gate::route::SplitMix64::seeded(7))
    };
    let allowed = |live: &GateLive| {
        let allowed = FollowUp::new(live, &candidates, (SONNET, None), workspace.path()).allowed;
        (allowed.review, allowed.criteria)
    };
    let (log, on, off) = (ReviewMode::Log, ReviewMode::On, ReviewMode::Off);
    assert_eq!(allowed(&gate(log, log, 1.0)), (log, log), "every attempt");
    assert_eq!(allowed(&gate(log, log, 0.0)), (off, off), "none");
    assert_eq!(
        allowed(&gate(on, log, 0.0)),
        (on, off),
        "a review Gate acts on is asked whatever the share"
    );
    let sampled = gate(log, off, 0.25);
    let asked = (0..200).filter(|_| allowed(&sampled).0 == log).count();
    assert!((30..=70).contains(&asked), "{asked} of 200");
}

#[test]
fn a_nudge_says_what_no_test_names() {
    let plain = nudge_message(&[]);
    assert!(plain.ends_with(NUDGE_PROMPT), "{plain}");
    let names = ["prepare_multipart".to_string(), "Part".to_string()];
    let told = nudge_message(&names);
    assert!(told.contains("`prepare_multipart`, `Part`"), "{told}");
    assert!(aura_gate::evidence::transcript::said::is_nudge(&told));
    let notice = nudge_notice(HAIKU, &names);
    assert!(
        notice.contains("none names `prepare_multipart` or 1 other"),
        "{notice}"
    );
}

fn check(passed: Option<bool>, exit_code: Option<i32>, command: &str, tail: &str) -> CheckRun {
    CheckRun {
        command: command.to_string(),
        passed,
        exit_code,
        tail: tail.to_string(),
    }
}

#[test]
fn a_check_that_ran_says_how_the_change_went() {
    let passed = check(Some(true), Some(0), "cargo test", "ok");
    assert_eq!(check_signal(&passed), Some(OutcomeSignal::ChecksPassed));
    let failed = check(
        Some(false),
        Some(101),
        "cargo test",
        "test parse ... FAILED",
    );
    assert_eq!(check_signal(&failed), Some(OutcomeSignal::ChecksFailed));
}

#[test]
fn a_check_that_could_not_run_says_nothing() {
    for unrun in [
        check(None, None, "cargo test", "denied"),
        check(Some(false), None, "cargo test", "Command timed out"),
        check(Some(false), Some(127), "go test ./...", "sh: go: not found"),
        check(
            Some(false),
            Some(1),
            "python -m pytest -q",
            "No module named pytest",
        ),
        check(Some(false), Some(5), "python -m pytest -q", "no tests ran"),
        check(
            Some(false),
            Some(1),
            "npm test --silent",
            "npm error Missing script: \"test\"",
        ),
    ] {
        assert_eq!(check_signal(&unrun), None, "{unrun:?}");
    }
}

#[test]
fn reviews_join_on_lines() {
    let some = |s: &str| Some(s.to_string());
    assert_eq!(joined(some("a"), some("b")), some("a\nb"));
    assert_eq!(joined(None, some("b")), some("b"));
    assert_eq!(joined(None, None), None);
}
