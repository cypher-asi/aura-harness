//! The judges of a turn: their requests, and the answers read out of
//! their replies.

use aura_gate_followup::Suspicion;
use aura_gate_route::Candidate;
use aura_model::model_catalog::{ModelBackend, ModelTier};
use aura_model::ToolChoice;

use super::failed::prompt;
use super::parts::MAX_FAILURE;
use super::unverified::unverified_prompt;
use super::{
    judges, parse_answer, request, review_prompt, unverified_request, Answer, Failed, Standing,
    Unverified, UNVERIFIED_SYSTEM_PROMPT,
};
use crate::criteria::{Criteria, Criterion, Kind};

fn failed() -> Failed<'static> {
    Failed {
        task: "fix the parser",
        diff: "--- src/parse.rs\n-old\n+new",
        reply: "Fixed it.",
        failure: "Check `cargo test` failed:\ntest parse ... FAILED",
    }
}

/// `reply` reads as `answer`, with `reasons`, for each case.
fn assert_answers(cases: &[(&str, Answer, &str)]) {
    for (reply, answer, reasons) in cases {
        let expected = Some((*answer, (*reasons).to_string()));
        assert_eq!(parse_answer(reply), expected, "{reply:?}");
    }
}

#[test]
fn the_verdict_is_the_last_verdict_line() {
    assert_answers(&[
        (
            "The loop is off by one.\nIt never handles n.\n\nVERDICT: MODEL",
            Answer::Model,
            "The loop is off by one.\nIt never handles n.",
        ),
        (
            "cargo is not installed.\n`verdict: other`\n",
            Answer::Other,
            "cargo is not installed.",
        ),
        (
            "<think>VERDICT: OTHER?</think>Wrong fix.\n**VERDICT: MODEL**",
            Answer::Model,
            "Wrong fix.",
        ),
        (
            "First VERDICT: OTHER, but then\nVERDICT: MODEL",
            Answer::Model,
            "First VERDICT: OTHER, but then",
        ),
    ]);
}

#[test]
fn work_found_done_answers_ok() {
    assert_answers(&[(
        "The change is complete.\nVERDICT: OK",
        Answer::Ok,
        "The change is complete.",
    )]);
}

#[test]
fn the_verdict_line_may_hold_its_label_anywhere() {
    assert_answers(&[
        (
            "Reasons\r\nmore\r\nThe VERDICT: ok.",
            Answer::Ok,
            "Reasons\nmore",
        ),
        ("VERDICT: MODEL VERDICT: OK", Answer::Model, ""),
        ("VERDICT: OK\nThanks!", Answer::Ok, ""),
    ]);
}

#[test]
fn a_reply_without_a_verdict_decides_nothing() {
    for reply in [
        "",
        "It looks wrong to me.",
        "VERDICT: MAYBE",
        "VERDICT:",
        "VERDICT: MODEL, mostly",
        "**Verdict**: MODEL",
        "VERDICT: MODEL\nVERDICT: unsure",
    ] {
        assert_eq!(parse_answer(reply), None, "{reply:?}");
    }
}

#[test]
fn the_unverified_request_shows_the_commands_and_what_looks_wrong() {
    let unverified = Unverified {
        task: "make ensure_type keep tags",
        diff: "",
        reply: "The code already does this.",
        commands: "rg ensure_type\ncat lib/config.py",
        last_check: "",
        suspicions: &[Suspicion::NoChanges],
    };
    let built = unverified_request(OPUS, &unverified, None).unwrap();
    assert_eq!(built.system, UNVERIFIED_SYSTEM_PROMPT);
    assert!(built.tools.is_empty());
    let sent = built.messages[0].text_content();
    for part in [
        "make ensure_type keep tags",
        "(no file changes recorded)",
        "cat lib/config.py",
        "The code already does this.",
        "## Its last test or build\n(it ran none)",
        Suspicion::NoChanges.describe(),
    ] {
        assert!(sent.contains(part), "{part}: {sent}");
    }
    assert!(unverified_request("", &unverified, None).is_err());
}

#[test]
fn the_unverified_request_shows_a_check_the_reply_disowns() {
    let unverified = Unverified {
        task: "make ensure_type keep tags",
        diff: "+    return value",
        reply: "I rewrote it, but it isn't verified.",
        commands: "pytest -q | head -20",
        last_check:
            "`pytest -q | head -20` exited with success. It printed:\nTypeError: boom\n9 passed",
        suspicions: &[Suspicion::Unverified],
    };
    let sent = unverified_prompt(&unverified);
    for part in [
        "## Its last test or build\n`pytest -q | head -20` exited with success.",
        "TypeError: boom",
        "it isn't verified",
        Suspicion::Unverified.describe(),
    ] {
        assert!(sent.contains(part), "{part}: {sent}");
    }
}

const OPUS: &str = "claude/claude-opus-4-7";

#[test]
fn the_request_shows_the_work_and_the_failure_without_tools() {
    let built = request("claude/claude-opus-4-7", &failed(), Some("jwt".into())).unwrap();
    assert_eq!(built.model.as_str(), "claude/claude-opus-4-7");
    assert!(built.tools.is_empty());
    assert!(matches!(built.tool_choice, ToolChoice::None));
    assert_eq!(built.auth_token.as_deref(), Some("jwt"));
    assert!(built.system.contains("VERDICT: MODEL"));
    let sent = built.messages[0].text_content();
    for part in [
        "fix the parser",
        "+new",
        "Fixed it.",
        "test parse ... FAILED",
    ] {
        assert!(sent.contains(part), "{part}: {sent}");
    }
    assert!(request("", &failed(), None).is_err());
}

#[test]
fn the_review_shows_the_criteria_it_is_given() {
    let why = ["the task was rated hard".to_string()];
    let mut standing = Standing {
        task: "add prepare_multipart",
        diff: "--- lib/urls.py\n+def prepare_multipart(fields):",
        whole: true,
        commands: "pytest test/urls",
        checks: "3 passed",
        reply: "Done.",
        why: &why,
        criteria: None,
    };
    let plain = review_prompt(&standing);
    assert!(!plain.contains("Acceptance criteria"), "{plain}");
    let criteria = Criteria {
        items: vec![Criterion {
            text: "the body is byte-identical to MIMEMultipart's".to_string(),
            kind: Kind::ReferenceMatch,
            reference: Some("email.mime.multipart".to_string()),
        }],
        strict: true,
    };
    standing.criteria = Some(&criteria);
    let held = review_prompt(&standing);
    let request = held.find("## The user's request").unwrap();
    let listed = held.find(&criteria.listed()).unwrap();
    let changes = held.find("## Changes").unwrap();
    assert!(request < listed && listed < changes, "{held}");
    assert!(held.contains("CRITERION <number>: MET"), "{held}");
}

#[test]
fn long_parts_are_cut_and_empty_ones_say_so() {
    let output = format!("{}the end", "x".repeat(MAX_FAILURE * 2));
    let long = Failed {
        diff: "",
        reply: " ",
        failure: &output,
        ..failed()
    };
    let sent = prompt(&long);
    assert!(sent.contains("(no file changes recorded)"));
    assert!(sent.contains("(no reply)"));
    assert!(sent.contains("(start cut)") && sent.ends_with("the end"));
    assert!(sent.len() < MAX_FAILURE + 500, "{}", sent.len());
}

#[test]
fn only_work_below_the_judges_tier_is_judged() {
    const HAIKU: &str = "claude/claude-haiku-4-5";
    const SONNET: &str = "claude/claude-sonnet-4-6";
    const OPUS: &str = "claude/claude-opus-4-7";
    assert!(judges(OPUS, HAIKU, &[]));
    assert!(judges(OPUS, SONNET, &[]));
    assert!(!judges(OPUS, OPUS, &[]), "nothing is stronger");
    assert!(!judges(SONNET, OPUS, &[]));
    assert!(!judges("", HAIKU, &[]), "no judge");
    let listed = [Candidate {
        id: "house-model".to_string(),
        label: "House".to_string(),
        backend: ModelBackend::Router,
        tier: ModelTier::Fast,
        cost_usd: None,
    }];
    assert!(
        judges(OPUS, "house-model", &listed),
        "a candidate's tier wins over its name"
    );
}
