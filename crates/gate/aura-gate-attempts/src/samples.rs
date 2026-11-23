//! The lines, logs and evidence the unit tests share, and the outputs
//! pinned for them in `fixtures/`.

use std::path::{Path, PathBuf};

use aura_core_types::{Difficulty, OutcomeSignal, PromptCategory};
use aura_gate_evidence::{CheckEvidence, Evidence, Flag};
use aura_gate_judge::criteria::{Checked, Criterion, Kind as CriterionKind, Status};
use aura_gate_judge::turn::Answer;

use crate::line::{Attempt, CopyAt, Line, Review, Truth, TruthSource};

/// The model the attempts run on unless a test says otherwise.
pub(crate) const SONNET: &str = "claude/claude-sonnet-5-5";

/// The strongest tier's model, which reviews unless a test says
/// otherwise.
pub(crate) const OPUS: &str = "claude/claude-opus-5-5";

/// A fast model.
const HAIKU: &str = "claude/claude-haiku-4-5";

/// The sample log: each kind of line as written today, a blank line,
/// lines older versions wrote, and two lines that are not lines of the
/// log.
pub(crate) const LOG: &str = include_str!("fixtures/log.jsonl");

/// [`tiered`]'s report as text.
pub(crate) const RENDERED: &str = include_str!("fixtures/rendered.txt");

/// [`sample`]'s report as `aura gate eval --json` prints it.
pub(crate) const REPORT: &str = include_str!("fixtures/report.json");

/// Where [`LOG`] is.
pub(crate) fn log_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fixtures/log.jsonl")
}

/// The evidence of an attempt that ran `command` once, which passed
/// printing `output`, changed a file and replied `reply`.
pub(crate) fn evidence(command: &str, output: &str, reply: &str) -> Evidence {
    Evidence {
        calls: 3,
        commands: 1,
        checks_run: 1,
        checks: vec![CheckEvidence {
            command: command.to_string(),
            passed: true,
            output: output.to_string(),
        }],
        files_changed: Some(1),
        reply: reply.to_string(),
        ..Evidence::default()
    }
}

/// Evidence that raises no sign.
pub(crate) fn clean() -> Evidence {
    evidence("pytest -q", "9 passed", "Done; the tests pass.")
}

/// Evidence of a check piped through `grep` that printed an error, and
/// a reply that says the work is not verified.
pub(crate) fn masked() -> Evidence {
    evidence(
        "pytest -q 2>&1 | grep -E 'passed|failed' | head -20",
        "TypeError: boom\n9 passed",
        "I rewrote it, but it isn't verified.",
    )
}

/// [`clean`], but with a file changed after the check.
pub(crate) fn stale() -> Evidence {
    Evidence {
        changed_after_check: true,
        ..clean()
    }
}

/// A hard attempt of agent `aa` at the prompt `prompt` that Gate routed
/// to Sonnet and that ended as `signal` says.
pub(crate) fn attempt(prompt: &str, signal: OutcomeSignal, evidence: Evidence) -> Attempt {
    Attempt {
        at_ms: 1,
        agent: "aa".to_string(),
        prompt_tx: prompt.to_string(),
        category: PromptCategory::MultiFileChange,
        difficulty: Some(Difficulty::Hard),
        led: Some(false),
        routed: true,
        model: SONNET.to_string(),
        signal,
        success: signal.success(),
        cost_usd: 0.1,
        subtask_cost_usd: 0.0,
        latency_ms: 48_000,
        evidence,
        backfilled: false,
    }
}

/// What `source` said of the work of [`attempt`]`(prompt, ..)`, a run of
/// `/compare` at a benchmark task.
pub(crate) fn truth(prompt: &str, source: TruthSource, passed: bool) -> Truth {
    Truth {
        at_ms: 2,
        agent: "aa".to_string(),
        prompt_tx: prompt.to_string(),
        source,
        passed,
        compare: Some("Compare #22".to_string()),
        run: Some("a-4".to_string()),
        task: Some("ansible-d33bedc4".to_string()),
        copy: None,
    }
}

/// [`truth`], for an attempt that was not a run of `/compare`: what the
/// user's rewind or retry said of it.
pub(crate) fn user_truth(prompt: &str, source: TruthSource, passed: bool) -> Truth {
    Truth {
        compare: None,
        run: None,
        task: None,
        ..truth(prompt, source, passed)
    }
}

/// A review by Opus, asked as `variant` by `aura gate replay`, of
/// [`attempt`]`(prompt, ..)`, which raised a shell edit.
pub(crate) fn review(prompt: &str, verdict: Answer, variant: &str) -> Review {
    Review {
        at_ms: 3,
        agent: "aa".to_string(),
        prompt_tx: prompt.to_string(),
        why: vec!["hard".to_string()],
        signs: vec![Flag::ShellEdit],
        verdict,
        findings: "The `int` case accepts `'1.1'`.".to_string(),
        judge: OPUS.to_string(),
        cost_usd: Some(0.05),
        variant: variant.to_string(),
        criteria: Vec::new(),
        acted: false,
        replayed: true,
    }
}

/// `count` attempts named `{name}{n}` with `evidence`, the first
/// `wrong` of them failing the task's check.
pub(crate) fn proven(name: &str, count: usize, wrong: usize, evidence: &Evidence) -> Vec<Line> {
    (0..count)
        .flat_map(|n| {
            let prompt = format!("{name}{n}");
            [
                Line::Attempt(attempt(
                    &prompt,
                    OutcomeSignal::ChecksPassed,
                    evidence.clone(),
                )),
                Line::Truth(truth(&prompt, TruthSource::TaskCheck, n >= wrong)),
            ]
        })
        .collect()
}

/// An attempt with every field a line can hold.
fn full_attempt() -> Attempt {
    let paths = vec!["src/app.py".to_string(), "tests/test_app.py".to_string()];
    Attempt {
        at_ms: 1_700_000_000_000,
        agent: "a1b2".to_string(),
        prompt_tx: "c3d4".to_string(),
        led: Some(true),
        cost_usd: 0.125,
        subtask_cost_usd: 0.25,
        evidence: Evidence {
            calls: 7,
            commands: 3,
            checks_run: 2,
            nudges: 1,
            files_edited: paths.clone(),
            files_written: vec!["tests/test_app.py".to_string()],
            files_changed: Some(2),
            changed_paths: Some(paths),
            changed_after_check: true,
            hit_limit: Some(false),
            ..evidence(
                "pytest -q",
                "9 passed",
                "Done; \"the tests\" pass.\nAll of them.",
            )
        },
        backfilled: true,
        ..attempt("p1", OutcomeSignal::ChecksPassed, Evidence::default())
    }
}

/// A review with every field a line can hold.
fn full_review() -> Review {
    let criterion = |text: &str, kind, reference: Option<&str>| Criterion {
        text: text.to_string(),
        kind,
        reference: reference.map(str::to_string),
    };
    Review {
        signs: vec![Flag::ShellEdit, Flag::StaleCheck],
        criteria: vec![
            Checked {
                criterion: criterion("Prints `ok`", CriterionKind::ExactOutput, Some("ok\n")),
                status: Some(Status::Unmet),
            },
            Checked {
                criterion: criterion("Raises on bad input", CriterionKind::Error, None),
                status: None,
            },
        ],
        acted: true,
        replayed: true,
        ..review("p1", Answer::Model, "plain+criteria")
    }
}

/// The lines [`LOG`] starts with: each kind with every field it can
/// hold, then with every field it can leave out.
pub(crate) fn written() -> Vec<Line> {
    let copy = |root: &str, git_dir: Option<&str>, baseline: &str| CopyAt {
        root: PathBuf::from(root),
        git_dir: git_dir.map(PathBuf::from),
        baseline: baseline.to_string(),
    };
    let kept = copy(
        "/home/me/.aura/compare/c1/a-4",
        Some("/home/me/.aura/compare/c1/a-4.git"),
        "4b825dc6",
    );
    vec![
        Line::Attempt(full_attempt()),
        Line::Attempt(Attempt {
            difficulty: None,
            led: None,
            routed: false,
            ..attempt("p2", OutcomeSignal::Unavailable, Evidence::default())
        }),
        Line::Truth(Truth {
            copy: Some(kept),
            ..truth("p1", TruthSource::TaskCheck, false)
        }),
        Line::Truth(Truth {
            copy: Some(copy("/work/wt", None, "abc")),
            ..truth("p1", TruthSource::TaskCheck, true)
        }),
        Line::Truth(user_truth("p1", TruthSource::RetryStronger, false)),
        Line::Review(full_review()),
        Line::Review(Review {
            signs: Vec::new(),
            replayed: false,
            ..review("p1", Answer::Other, "plain")
        }),
    ]
}

/// The lines of [`LOG`] older versions wrote, as they read today.
pub(crate) fn older() -> Vec<Line> {
    vec![
        Line::Attempt(Attempt {
            at_ms: 5,
            difficulty: None,
            led: None,
            model: "claude/claude-sonnet-4-5".to_string(),
            cost_usd: 0.5,
            latency_ms: 1_000,
            evidence: Evidence {
                calls: 1,
                reply: "Done.".to_string(),
                ..Evidence::default()
            },
            ..attempt("p0", OutcomeSignal::Unavailable, Evidence::default())
        }),
        Line::Truth(Truth {
            at_ms: 6,
            compare: Some("Compare #3".to_string()),
            run: Some("b-1".to_string()),
            task: None,
            ..truth("p0", TruthSource::Picked, true)
        }),
    ]
}

/// Attempts Gate let stand, wrong and right, flagged or not, one it had
/// nothing to judge by, one it counted as failed, and one nothing proved.
pub(crate) fn flagged() -> Vec<Line> {
    vec![
        // Let stand, wrong, and flagged.
        Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, masked())),
        Line::Truth(truth("p1", TruthSource::TaskCheck, false)),
        // Let stand and right: clean, then one that pipes its check.
        Line::Attempt(attempt("p2", OutcomeSignal::ChecksPassed, clean())),
        Line::Truth(truth("p2", TruthSource::TaskCheck, true)),
        Line::Attempt(attempt(
            "p3",
            OutcomeSignal::ChecksPassed,
            evidence("pytest -q | tail -5", "9 passed", "Done."),
        )),
        Line::Truth(truth("p3", TruthSource::TaskCheck, true)),
        // Nothing to judge by, wrong.
        Line::Attempt(attempt(
            "p4",
            OutcomeSignal::Unavailable,
            Evidence::default(),
        )),
        Line::Truth(truth("p4", TruthSource::Rewind, false)),
        // Counted as failed (and moved up): not what flags are for.
        Line::Attempt(attempt("p5", OutcomeSignal::ChecksFailed, clean())),
        Line::Truth(truth("p5", TruthSource::TaskCheck, true)),
        // No truth: not scored.
        Line::Attempt(attempt("p6", OutcomeSignal::ChecksPassed, masked())),
    ]
}

/// A log with every part of a report in it: [`sample_attempts`], then
/// reviews of some of them.
pub(crate) fn sample() -> Vec<Line> {
    let mut lines = sample_attempts();
    lines.extend([
        Line::Review(review("p1", Answer::Model, "plain")),
        Line::Review(review("p2", Answer::Ok, "plain")),
        Line::Review(review("p4", Answer::Model, "plain")),
        // Sent back, by another reviewer asked another way.
        Line::Review(Review {
            judge: SONNET.to_string(),
            cost_usd: Some(0.125),
            signs: vec![Flag::StaleCheck],
            acted: true,
            replayed: false,
            ..review("p5", Answer::Model, "thinking")
        }),
        // Of an attempt the log does not hold.
        Line::Review(review("gone", Answer::Model, "plain")),
    ]);
    lines
}

/// Attempts of each kind a report tells apart, and what proved them.
fn sample_attempts() -> Vec<Line> {
    vec![
        // Let stand, wrong, and flagged.
        Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, masked())),
        Line::Truth(truth("p1", TruthSource::TaskCheck, false)),
        // Let stand and right.
        Line::Attempt(attempt("p2", OutcomeSignal::ChecksPassed, clean())),
        Line::Truth(truth("p2", TruthSource::TaskCheck, true)),
        // Another model's work, right but flagged.
        Line::Attempt(Attempt {
            model: HAIKU.to_string(),
            ..attempt(
                "p3",
                OutcomeSignal::ChecksPassed,
                evidence("pytest -q | tail -5", "9 passed", "Done."),
            )
        }),
        Line::Truth(truth("p3", TruthSource::TaskCheck, true)),
        // Nothing to judge by, and the user rewound it.
        Line::Attempt(attempt(
            "p4",
            OutcomeSignal::Unavailable,
            Evidence::default(),
        )),
        Line::Truth(user_truth("p4", TruthSource::Rewind, false)),
        // Wrong, with no sign raised.
        Line::Attempt(attempt("p5", OutcomeSignal::ChecksPassed, clean())),
        Line::Truth(truth("p5", TruthSource::TaskCheck, false)),
        // Counted as failed, and right.
        Line::Attempt(attempt("p6", OutcomeSignal::ChecksFailed, clean())),
        Line::Truth(truth("p6", TruthSource::TaskCheck, true)),
        // On the strongest tier, and not routed.
        Line::Attempt(Attempt {
            model: OPUS.to_string(),
            routed: false,
            ..attempt("p7", OutcomeSignal::ChecksPassed, stale())
        }),
        Line::Truth(truth("p7", TruthSource::TaskCheck, true)),
        // Nothing proved it.
        Line::Attempt(attempt("p8", OutcomeSignal::ChecksPassed, masked())),
    ]
}

/// [`sample`] with enough attempts for a sign in each tier.
pub(crate) fn tiered() -> Vec<Line> {
    let mut lines = sample();
    lines.extend(proven("m", 10, 8, &masked()));
    lines.extend(proven("s", 6, 2, &stale()));
    lines.extend(proven("c", 20, 0, &clean()));
    lines
}
