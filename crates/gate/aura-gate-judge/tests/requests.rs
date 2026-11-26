//! Every request the judge crate builds, pinned byte for byte, with the
//! worker's note and the JSON Gate writes of criteria and answers.
//!
//! Each request is compared in full, as its `Debug` output, with the file
//! of the same name in `tests/pinned/`. A request whose parts run past
//! their limits is tens of thousands of characters long, so its file holds
//! the length and a digest of that output instead. After changing one on
//! purpose, run these tests with `UPDATE_PINS=1` to write what they build
//! over the files, and review the difference.

// Integration-test crate: test code may unwrap/expect.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![deny(clippy::too_many_lines)]

use std::fmt::Debug;
use std::path::PathBuf;

use aura_gate_followup::Suspicion;
use aura_gate_judge::blind::Attempt;
use aura_gate_judge::criteria::{Checked, Criteria, Criterion, Kind, Status};
use aura_gate_judge::turn::{Answer, Failed, ReviewVariant, Standing, Unverified};
use aura_gate_judge::{blind, classify, criteria, turn};

const HAIKU: &str = "claude/claude-haiku-5-5";
const SONNET: &str = "claude/claude-sonnet-5-5";
const OPUS: &str = "claude/claude-opus-4-7";
const GPT: &str = "chatgpt/gpt-5.4";
const LOCAL: &str = "local/qwen3:4b";

/// A request with what normalizing and fencing change: carriage returns,
/// runs of blank lines, trailing spaces, a control character and tags.
const TASK: &str = "Fix the parser.\r\n\r\n\r\nIt drops the last token  \u{7}\n\
                    Keep <request> and </REQUEST> as they are.\n\n";

/// `built` as pinned in `tests/pinned/<name>.txt`.
fn pinned(name: &str, built: &str) {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "pinned", name]
        .iter()
        .collect();
    let path = path.with_extension("txt");
    if std::env::var_os("UPDATE_PINS").is_some() {
        std::fs::write(&path, built).unwrap();
    }
    let pinned =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(built, pinned, "{name} differs from {}", path.display());
}

/// `value`'s `Debug` output, as pinned.
fn shown(value: &impl Debug) -> String {
    format!("{value:#?}\n")
}

/// The length of `value`'s `Debug` output and its FNV-1a digest.
fn digest(value: &impl Debug) -> String {
    let text = shown(value);
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{} bytes, FNV-1a {hash:#018x}\n", text.len())
}

/// `lines` numbered lines of `what`, each 50 two-byte characters longer,
/// for parts that run past every limit.
fn long(what: &str, lines: usize) -> String {
    (0..lines)
        .map(|i| format!("{what} {i:05} {}", "é".repeat(50)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parts of a turn longer than their limits.
struct Long {
    task: String,
    diff: String,
    reply: String,
    output: String,
}

impl Long {
    fn new() -> Self {
        Self {
            task: long("Task", 200),
            diff: long("+diff", 1_300),
            reply: long("Reply", 100),
            output: long("Output", 150),
        }
    }
}

fn criteria_list() -> Criteria {
    Criteria {
        items: vec![
            Criterion {
                text: "prepare_multipart is importable from ansible.module_utils.urls".to_string(),
                kind: Kind::Interface,
                reference: None,
            },
            Criterion {
                text: "The body is byte-identical to MIMEMultipart's".to_string(),
                kind: Kind::ReferenceMatch,
                reference: Some("email.mime.multipart with email.policy.HTTP".to_string()),
            },
        ],
        strict: true,
    }
}

#[test]
fn classifier_requests() {
    for (name, model, token) in [
        ("classify_tool", HAIKU, Some("jwt".to_string())),
        ("classify_text", LOCAL, None),
    ] {
        pinned(
            name,
            &shown(&classify::request(model, TASK, token).unwrap()),
        );
    }
    let prompt = long("Prompt line", 400);
    for (name, model) in [("classify_tool_long", HAIKU), ("classify_text_long", LOCAL)] {
        pinned(
            name,
            &digest(&classify::request(model, &prompt, None).unwrap()),
        );
    }
}

#[test]
fn criteria_requests() {
    for (name, model, token) in [
        ("criteria_auto", SONNET, Some("jwt".to_string())),
        ("criteria_forced", GPT, None),
        ("criteria_text", LOCAL, None),
    ] {
        pinned(
            name,
            &shown(&criteria::request(model, TASK, token).unwrap()),
        );
    }
    let prompt = long("Requirement", 1_000);
    for (name, model) in [
        ("criteria_auto_long", SONNET),
        ("criteria_text_long", LOCAL),
    ] {
        pinned(
            name,
            &digest(&criteria::request(model, &prompt, None).unwrap()),
        );
    }
}

fn failed() -> Failed<'static> {
    Failed {
        task: TASK,
        diff: "--- src/parse.rs\n+++ src/parse.rs\n-    tokens.pop();\n+    tokens.last();",
        reply: "Fixed the parser: it keeps the last token now.",
        failure: "Check `cargo test` failed:\ntest parse::keeps_last ... FAILED",
    }
}

#[test]
fn failed_turn_requests() {
    let built = turn::request(OPUS, &failed(), Some("jwt".to_string())).unwrap();
    pinned("turn_failed", &shown(&built));
    let blank = Failed {
        diff: "",
        reply: " ",
        failure: "\n",
        ..failed()
    };
    pinned(
        "turn_failed_blank",
        &shown(&turn::request(OPUS, &blank, None).unwrap()),
    );
    let long = Long::new();
    let past = Failed {
        task: &long.task,
        diff: &long.diff,
        reply: &long.reply,
        failure: &long.output,
    };
    pinned(
        "turn_failed_long",
        &digest(&turn::request(OPUS, &past, None).unwrap()),
    );
}

fn unverified() -> Unverified<'static> {
    Unverified {
        task: TASK,
        diff: "+    return value",
        reply: "I rewrote it, but it isn't verified.",
        commands: "rg ensure_type\npytest -q | head -20",
        last_check: "`pytest -q | head -20` exited with success. It printed:\n9 passed",
        suspicions: &[Suspicion::NoChanges, Suspicion::Unverified],
    }
}

#[test]
fn unverified_turn_requests() {
    let built = turn::unverified_request(OPUS, &unverified(), Some("jwt".to_string())).unwrap();
    pinned("turn_unverified", &shown(&built));
    let blank = Unverified {
        diff: " ",
        reply: "",
        commands: "",
        last_check: "\t",
        suspicions: &[],
        ..unverified()
    };
    let built = turn::unverified_request(OPUS, &blank, None).unwrap();
    pinned("turn_unverified_blank", &shown(&built));
    let long = Long::new();
    let past = Unverified {
        task: &long.task,
        diff: &long.diff,
        reply: &long.reply,
        commands: &long.output,
        last_check: &long.output,
        suspicions: &[Suspicion::GaveUp],
    };
    let built = turn::unverified_request(OPUS, &past, None).unwrap();
    pinned("turn_unverified_long", &digest(&built));
}

const WHY: [&str; 2] = ["the task was rated hard", "its reply says it gave up"];

fn standing<'a>(why: &'a [String], criteria: &'a Criteria) -> Standing<'a> {
    Standing {
        task: TASK,
        diff: "--- lib/urls.py\n+def prepare_multipart(fields):",
        whole: true,
        commands: "pytest test/urls",
        checks: "`pytest test/urls` exited with success. It printed:\n3 passed",
        reply: "Done.",
        why,
        criteria: Some(criteria),
    }
}

#[test]
fn standing_work_reviews() {
    let why = WHY.map(str::to_string);
    let criteria = criteria_list();
    let standing = standing(&why, &criteria);
    let (plain, thinking) = (ReviewVariant::Plain, ReviewVariant::Thinking);
    let built = turn::review_request(OPUS, &standing, plain, Some("jwt".to_string())).unwrap();
    pinned("turn_review_plain", &shown(&built));
    let blank = Standing {
        diff: "",
        whole: false,
        commands: " ",
        checks: "",
        reply: "",
        why: &[],
        criteria: None,
        ..standing
    };
    let built = turn::review_request(OPUS, &blank, thinking, None).unwrap();
    pinned("turn_review_thinking", &shown(&built));
}

#[test]
fn a_review_of_long_work() {
    let why = WHY.map(str::to_string);
    let criteria = criteria_list();
    let long = Long::new();
    let past = Standing {
        task: &long.task,
        diff: &long.diff,
        commands: &long.output,
        checks: &long.output,
        reply: &long.reply,
        ..standing(&why, &criteria)
    };
    let built = turn::review_request(OPUS, &past, ReviewVariant::Plain, None).unwrap();
    pinned("turn_review_long", &digest(&built));
}

fn attempt(diff: &str, reply: &str) -> Attempt {
    Attempt {
        diff: diff.to_string(),
        reply: reply.to_string(),
    }
}

#[test]
fn blind_judge_requests() {
    let a = attempt("+fn parse() {}", "Fixed the parser.");
    let b = attempt("+fn parse_all() {}\n", "Rewrote parsing.");
    let token = Some("jwt".to_string());
    let built = blind::request(OPUS, TASK, (&a, &b), true, None, token).unwrap();
    pinned("blind_a_first", &shown(&built));
    let blank = attempt(" ", "\n");
    let kept = attempt("+fn kept() {}", "Kept this.");
    let built = blind::request(OPUS, TASK, (&a, &blank), false, Some(&kept), None).unwrap();
    pinned("blind_b_first_reference", &shown(&built));
    let long = Long::new();
    let past = attempt(&long.diff, &long.reply);
    let built = blind::request(OPUS, TASK, (&past, &b), true, Some(&past), None).unwrap();
    pinned("blind_long", &digest(&built));
}

#[test]
fn the_workers_note_and_what_is_written_of_criteria() {
    let criteria = criteria_list();
    let note = criteria::worker_note(&criteria);
    pinned("criteria_worker_note", &format!("{note}\n"));
    let findings = "CRITERION 1: MET, test_imports\nCRITERION 2: UNTESTED";
    let mut checked: Vec<Checked> = criteria::checked(&criteria, findings);
    checked.push(Checked {
        criterion: criteria.items[0].clone(),
        status: None,
    });
    let written = [
        serde_json::to_string(&criteria).unwrap(),
        serde_json::to_string(&Criteria::default()).unwrap(),
        serde_json::to_string(&checked).unwrap(),
        serde_json::to_string(&[Answer::Model, Answer::Ok, Answer::Other]).unwrap(),
        serde_json::to_string(&[Status::Met, Status::Unmet, Status::Untested]).unwrap(),
    ];
    pinned("criteria_json", &format!("{}\n", written.join("\n")));
}
