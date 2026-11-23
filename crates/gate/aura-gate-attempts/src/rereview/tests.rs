use super::*;
use crate::samples::{self, OPUS, SONNET};
use crate::{Attempt, Review, Tiers, Truth};
use aura_core_types::OutcomeSignal;
use aura_gate_evidence::{Evidence, Flag};
use aura_gate_followup::Why;
use aura_gate_judge::turn::Answer;
use aura_model::{
    ContentBlock, Message, ModelResponse, ProviderTrace, Role, StopReason, ToolResultContent, Usage,
};
use serde_json::json;
use std::path::Path;

/// A hard attempt by `agent` at `prompt` that Gate routed to `model` and
/// let stand.
fn attempt(agent: &str, prompt: &str, model: &str) -> Attempt {
    Attempt {
        agent: agent.to_string(),
        model: model.to_string(),
        ..samples::attempt(prompt, OutcomeSignal::ChecksPassed, Evidence::default())
    }
}

/// What a benchmark task's check said of `attempt`, a run of `/compare`
/// whose copy is kept under `root` when the log says where.
fn truth(attempt: &Attempt, passed: bool, root: Option<&str>) -> Truth {
    Truth {
        agent: attempt.agent.clone(),
        run: Some("a-6".to_string()),
        task: Some("ansible-d72025be".to_string()),
        copy: root.map(|root| CopyAt {
            root: Path::new(root).join("a-6"),
            git_dir: Some(Path::new(root).join("a-6.git")),
            baseline: String::new(),
        }),
        ..samples::truth(&attempt.prompt_tx, TruthSource::TaskCheck, passed)
    }
}

/// Whether a copy is still kept: those under `/kept` are.
fn kept(copy: &CopyAt) -> bool {
    copy.root.starts_with("/kept")
}

/// A saved session whose run changed `lib.py` with a shell command and
/// then ran its tests.
fn session() -> Vec<Message> {
    let call = |id: &str, command: &str| {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: "run_command".to_string(),
                input: json!({ "command": command }),
            }],
        )
    };
    let result = |id: &str, output: &str| {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: ToolResultContent::Text(output.to_string()),
                is_error: false,
            }],
        )
    };
    vec![
        Message::user("Make f() return 2."),
        call("1", "sed -i 's/return 1/return 2/' lib.py"),
        result("1", ""),
        call("2", "python -m pytest -q test"),
        result("2", "1 passed in 0.01s"),
        reply("Done; the tests pass."),
    ]
}

/// The model's reply `text`.
fn reply(text: &str) -> Message {
    Message::new(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: text.to_string(),
        }],
    )
}

/// What [`session`]'s run changed in its copy.
const DIFF: &str = "diff --git a/lib.py b/lib.py\n--- a/lib.py\n+++ b/lib.py\n\
                    @@ -1,2 +1,2 @@\n def f():\n-    return 1\n+    return 2\n";

/// [`session`]'s attempt, hard and routed to Sonnet, as the reviewer is
/// shown it.
fn prepared() -> Prepared {
    let line = attempt("aa", "p0", SONNET);
    let lines = [
        Line::Attempt(line.clone()),
        Line::Truth(truth(&line, false, Some("/kept"))),
    ];
    let (picks, _) = pick(&lines, &Filter::default(), &HashSet::new(), kept);
    let messages = session();
    let task = "Make f() return 2.".to_string();
    let changed = ["lib.py".to_string()];
    Prepared::of(
        &picks[0],
        (task, &messages),
        DIFF.to_string(),
        &changed,
        &Tiers::default(),
    )
}

/// A log of one attempt that can be replayed, `a0` at `p0`, then one
/// left out for each reason but the filter's: an easy one, one on Opus,
/// one Gate did not route, two by an agent that moved, one whose copy
/// the log does not place and one whose copy is gone.
fn log_of_each_kind() -> Vec<Line> {
    let easy = Attempt {
        difficulty: Some(Difficulty::Easy),
        ..attempt("a1", "p1", SONNET)
    };
    let pinned = Attempt {
        routed: false,
        ..attempt("a3", "p3", SONNET)
    };
    let kept_under = |attempt: Attempt, root| {
        let proved = truth(&attempt, false, root);
        [Line::Attempt(attempt), Line::Truth(proved)]
    };
    [
        kept_under(attempt("a0", "p0", SONNET), Some("/kept")),
        kept_under(easy, Some("/kept")),
        kept_under(attempt("a2", "p2", OPUS), Some("/kept")),
        kept_under(pinned, Some("/kept")),
        kept_under(attempt("a5", "p5", SONNET), Some("/kept")),
        kept_under(attempt("a5", "p6", SONNET), Some("/kept")),
        kept_under(attempt("a4", "p4", SONNET), None),
        kept_under(attempt("a6", "p7", SONNET), Some("/removed")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[test]
fn only_standing_hard_routed_work_with_a_kept_copy_is_picked() {
    let lines = log_of_each_kind();
    let (picks, left_out) = pick(&lines, &Filter::default(), &HashSet::new(), kept);
    assert_eq!(picks.len(), 1, "{left_out:?}");
    assert_eq!(picks[0].stood.attempt.agent, "a0");
    let left: Vec<(&str, usize)> = left_out.iter().map(|(why, n)| (*why, *n)).collect();
    assert_eq!(
        left,
        [
            ("Gate did not choose their model", 1),
            ("not rated hard (--all takes them)", 1),
            ("on the strongest tier", 1),
            (COPY_NOT_NOTED, 1),
            ("their run moved between models or prompts", 2),
            ("their run's copy is not kept", 1),
        ]
    );
    assert_eq!(strongest_in(&lines).as_deref(), Some(OPUS));
    assert_eq!(writer_in(&lines).as_deref(), Some(SONNET));
}

#[test]
fn any_difficulty_a_task_a_limit_and_asking_again_are_on_request() {
    let lines = log_of_each_kind();
    let filter = Filter::default();
    let any = Filter {
        any_difficulty: true,
        ..filter
    };
    assert_eq!(pick(&lines, &any, &HashSet::new(), kept).0.len(), 2);
    let one = Filter {
        limit: Some(1),
        ..any
    };
    assert_eq!(pick(&lines, &one, &HashSet::new(), kept).0.len(), 1);
    let done = HashSet::from([("a0", "p0")]);
    assert!(pick(&lines, &filter, &done, kept).0.is_empty());
    let again = Filter {
        again: true,
        ..filter
    };
    assert_eq!(pick(&lines, &again, &done, kept).0.len(), 1);
    let other_task = Filter {
        task: Some("element-web"),
        ..filter
    };
    let (picks, left_out) = pick(&lines, &other_task, &HashSet::new(), kept);
    assert!(picks.is_empty() && left_out.is_empty());
}

#[test]
fn what_a_reviewer_was_asked_again_the_same_way_is_replayed() {
    let live = Review {
        replayed: false,
        ..samples::review("p1", Answer::Model, "plain")
    };
    let lines = [
        Line::Review(samples::review("p0", Answer::Model, "plain")),
        Line::Review(samples::review("p2", Answer::Ok, "plain+criteria")),
        Line::Review(live),
    ];
    let held = asked_as(ReviewVariant::Plain, true);
    assert_eq!(
        replayed(&lines, "plain", Some(OPUS)),
        HashSet::from([("aa", "p0")])
    );
    assert_eq!(
        replayed(&lines, &held, Some(OPUS)),
        HashSet::from([("aa", "p2")])
    );
    assert!(replayed(&lines, "plain", Some(SONNET)).is_empty());
    assert!(replayed(&lines, "plain", None).is_empty());
}

#[test]
fn the_reviewer_is_shown_what_the_run_left_and_its_review_is_a_line_of_the_log() {
    let ready = prepared();
    assert_eq!(
        ready.attempt.evidence.changed_paths.as_deref(),
        Some(&["lib.py".to_string()][..])
    );
    assert_eq!(
        ready.signs,
        [Flag::ShellEdit, Flag::NoTestChanged],
        "the change was made by a command and touched no test"
    );
    assert_eq!(
        ready.why,
        [("hard".to_string(), Why::Hard.describe().to_string())]
    );
    assert!(ready.commands.contains("sed -i"), "{}", ready.commands);
    assert!(ready.checks.contains("1 passed"), "{}", ready.checks);
    assert_eq!(ready.reply, "Done; the tests pass.");
    assert_eq!(
        (ready.run.as_str(), ready.task_id.as_str(), ready.right),
        ("Compare #22 a-6", "ansible-d72025be", false)
    );
    assert!(estimate(OPUS, std::slice::from_ref(&ready), ReviewVariant::Plain).is_some());
    assert_eq!(
        ready.listed(),
        "  Compare #22 a-6  ansible-d72025be                         \
         claude-sonnet-5-5 (claude plan)  wrong  1 files, 109 chars of diff  \
         shell_edit no_test_changed"
    );

    let answered = (Answer::Model, "1. Negative input is not rejected.", 0.05);
    let Line::Review(review) = ready.review(OPUS, ReviewVariant::Plain, answered, 9) else {
        panic!("a review is a review line");
    };
    assert!(review.replayed && !review.acted);
    assert_eq!(
        (review.at_ms, review.variant.as_str(), review.judge.as_str()),
        (9, "plain", OPUS)
    );
    assert_eq!(review.why, ["hard"]);
    assert_eq!(review.signs, [Flag::ShellEdit, Flag::NoTestChanged]);
    assert_eq!(
        (review.agent.as_str(), review.prompt_tx.as_str()),
        ("aa", "p0")
    );
}

/// The reviewer's reply `text`, which stopped for `stop_reason`.
fn response(text: &str, stop_reason: StopReason) -> ModelResponse {
    let usage = Usage::new(1_000, 200);
    ModelResponse::new(stop_reason, reply(text), usage, ProviderTrace::default())
}

#[test]
fn a_reply_is_an_answer_and_one_without_a_verdict_says_why() {
    let ready = prepared();
    let reply = response("1. Negative input.\nVERDICT: MODEL", StopReason::EndTurn);
    let answered = Asked::of(&ready, OPUS, &reply);
    assert!(
        matches!(&answered.answer, Ok((Answer::Model, findings, cost))
            if findings.contains("Negative input") && *cost > 0.0),
        "{answered:?}"
    );
    assert_eq!((answered.unreached(), answered.wasted_usd), (None, 0.0));

    let cut = Asked::of(&ready, OPUS, &response("Looking at", StopReason::MaxTokens));
    assert!(
        matches!(&cut.answer, Err(problem) if problem.starts_with(
            "the reviewer gave no verdict: its reply was cut off at the token limit after 200 tokens"
        )),
        "{cut:?}"
    );
    assert!(cut.wasted_usd > 0.0);
    assert_eq!(cut.unreached(), None, "the reviewer was reached");

    let failed = Asked::failed(&ready, "the reviewer took too long".to_string());
    assert_eq!(failed.unreached(), Some("the reviewer took too long"));
}

/// Four reviews of Sonnet's work: wrong work found unfinished while held
/// to criteria, right work found done, right work held up, and wrong
/// work whose reply named no verdict.
fn four_reviews() -> [Asked; 4] {
    let asked = |run: &str, right: bool, answer, wasted_usd, criteria: &str| Asked {
        run: run.to_string(),
        task_id: "ansible-d72025be".to_string(),
        model: "Sonnet 5.5".to_string(),
        right,
        answer,
        wasted_usd,
        criteria: criteria.to_string(),
    };
    let findings = "1. The fix misses the empty case.\n\n2. No test covers it.  \n3. c\n\
                    4. d\n5. e\n6. f\n7. g\n";
    [
        asked(
            "Compare #22 a-6",
            false,
            Ok((Answer::Model, findings.to_string(), 0.0123)),
            0.0,
            ", criteria 1/2 met, 1 unmet",
        ),
        asked(
            "Compare #22 b-6",
            true,
            Ok((Answer::Ok, "Every requirement is met.".to_string(), 0.02)),
            0.0,
            "",
        ),
        asked(
            "Compare #23 a-1",
            true,
            Ok((Answer::Other, String::new(), 0.004)),
            0.0,
            "",
        ),
        asked(
            "Compare #23 b-1",
            false,
            Err("the reviewer gave no verdict: its reply ended without one".to_string()),
            0.001,
            "",
        ),
    ]
}

#[test]
fn each_review_is_a_row_and_the_reviews_are_tallied() {
    let all = four_reviews();
    let rows: Vec<String> = all.iter().map(Asked::row).collect();
    assert_eq!(
        rows,
        [
            "  Compare #22 a-6  ansible-d72025be                         Sonnet 5.5                       wrong  unfinished  $0.012, criteria 1/2 met, 1 unmet\n      1. The fix misses the empty case.\n      2. No test covers it.\n      3. c\n      4. d\n      5. e\n      6. f",
            "  Compare #22 b-6  ansible-d72025be                         Sonnet 5.5                       right  done        $0.020",
            "  Compare #23 a-1  ansible-d72025be                         Sonnet 5.5                       right  held up     $0.004",
            "  Compare #23 b-1  ansible-d72025be                         Sonnet 5.5                       wrong  no answer: the reviewer gave no verdict: its reply ended without one",
        ]
    );
    assert_eq!(
        tally(&all),
        "3 reviews answered, 1 not, $0.04 at list price\n\
         wrong work: 1 of 1 found unfinished\n\
         right work: 0 of 2 found unfinished"
    );
}
