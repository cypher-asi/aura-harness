use super::*;
use crate::isolation::changes;
use crate::session_checkpoint::{Checkpoints, SessionCheckpoint};
use aura_core_types::{AgentId, Difficulty, Hash, OutcomePayload, OutcomeSignal, PromptCategory};
use aura_gate::attempts::rereview::Asked;
use aura_gate::attempts::{Attempt, Terms, Tiers, Truth, TruthSource};
use aura_gate::evidence::Flag;
use aura_gate::followup::Why;
use aura_gate::judge::criteria;
use aura_gate::judge::turn::{self, Answer};
use aura_model::{ContentBlock, Message, Role, ToolResultContent};
use serde_json::json;
use std::collections::HashSet;
use std::path::Path;
use tempfile::TempDir;

const SONNET: &str = "claude/claude-sonnet-5-5";
const OPUS: &str = "claude/claude-opus-5-5";

fn attempt(agent: AgentId, n: u8, model: &str, difficulty: Option<Difficulty>) -> Attempt {
    let ended = OutcomePayload {
        prompt_tx: Hash::from_content(&[n]),
        category: PromptCategory::Debugging,
        model: model.to_string(),
        success: Some(true),
        signal: OutcomeSignal::ChecksPassed,
        cost_usd: 0.1,
        subtask_cost_usd: 0.0,
        latency_ms: 10,
    };
    let terms = Terms {
        difficulty,
        led: Some(false),
        routed: true,
    };
    let mut attempt = Attempt::new(1, agent.to_hex(), &ended, terms, Evidence::default());
    attempt.backfilled = true;
    attempt
}

fn truth(attempt: &Attempt, passed: bool) -> Truth {
    Truth {
        at_ms: 2,
        agent: attempt.agent.clone(),
        prompt_tx: attempt.prompt_tx.clone(),
        source: TruthSource::TaskCheck,
        passed,
        compare: Some("Compare #22".to_string()),
        run: Some("a-6".to_string()),
        task: Some("ansible-d72025be".to_string()),
        copy: None,
    }
}

/// A kept copy under `root` whose run changed `lib.py` with a shell
/// command, and the place it is kept at.
fn kept_copy(root: &Path) -> Option<CopyAt> {
    let copy = root.join("a-6");
    let git_dir = root.join("a-6.git");
    std::fs::create_dir_all(copy.join("test")).unwrap();
    std::fs::write(copy.join("lib.py"), "def f():\n    return 1\n").unwrap();
    std::fs::write(copy.join("test/test_lib.py"), "def test_f(): pass\n").unwrap();
    let baseline = changes::baseline(&copy, Some(&git_dir)).ok()?;
    std::fs::write(copy.join("lib.py"), "def f():\n    return 2\n").unwrap();
    Some(CopyAt {
        root: copy,
        git_dir: Some(git_dir),
        baseline,
    })
}

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
        Message::new(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "Done; the tests pass.".to_string(),
            }],
        ),
    ]
}

/// A hard attempt whose run's copy is kept under `dir` and whose session
/// is saved there, as the reviewer is shown it; `None` where git is not
/// there to keep a copy's baseline.
async fn kept_attempt(dir: &TempDir) -> Option<Prepared> {
    let copy = kept_copy(dir.path())?;
    let agent = AgentId::generate();
    let line = attempt(agent, 0, SONNET, Some(Difficulty::Hard));
    let proved = Truth {
        copy: Some(copy),
        ..truth(&line, false)
    };
    let sessions = Checkpoints::new(dir.path());
    let saved = SessionCheckpoint::new(5, 7, session(), Vec::new(), Default::default());
    sessions.save(agent, &saved).await.unwrap();
    let lines = [Line::Attempt(line), Line::Truth(proved)];
    let kept = |copy: &CopyAt| copy.root.is_dir();
    let (picks, _) = pick(&lines, &Filter::default(), &HashSet::new(), kept);
    Some(prepare::prepare(&picks[0], &sessions, &Tiers::default()).unwrap())
}

/// The reviewer `judge` answering `reply` through a mock, logging to the
/// attempt log under `dir`.
fn reviewer(dir: &TempDir, judge: &str, reply: &str) -> Reviewer {
    Reviewer {
        provider: Arc::new(aura_model::MockProvider::simple_response(reply)),
        auth_token: None,
        judge: judge.to_string(),
        variant: ReviewVariant::Plain,
        log: Arc::new(AttemptLog::in_data_dir(dir.path())),
    }
}

#[tokio::test]
async fn the_reviewer_is_shown_what_the_run_left() {
    let dir = TempDir::new().unwrap();
    let Some(ready) = kept_attempt(&dir).await else {
        return;
    };
    assert_eq!(ready.task, "Make f() return 2.");
    assert!(
        ready.diff.contains("-    return 1\n+    return 2"),
        "{}",
        ready.diff
    );
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
        (ready.run.as_str(), ready.right),
        ("Compare #22 a-6", false)
    );
}

#[tokio::test]
async fn the_reviewers_answer_goes_to_the_log_marked_as_replayed() {
    let dir = TempDir::new().unwrap();
    let Some(ready) = kept_attempt(&dir).await else {
        return;
    };
    let reply = "1. `f` is not covered for negative input.\nVERDICT: MODEL";
    let reviewer = reviewer(&dir, OPUS, reply);
    let prompt_tx = ready.attempt.prompt_tx.clone();
    let asked = reviewer.ask(ready).await;
    assert!(
        matches!(&asked.answer, Ok((Answer::Model, _, _))),
        "{asked:?}"
    );
    let shown = asked.row();
    assert!(shown.contains("wrong  unfinished"), "{shown}");
    assert!(shown.contains("1. `f` is not covered"), "{shown}");
    let written = reviewer.log.read().unwrap().lines;
    let [Line::Review(review)] = &written[..] else {
        panic!("{written:?}");
    };
    assert!(review.replayed && !review.acted);
    assert_eq!(
        (review.variant.as_str(), review.judge.as_str()),
        ("plain", OPUS)
    );
    assert_eq!(review.why, ["hard"]);
    assert_eq!(review.signs, [Flag::ShellEdit, Flag::NoTestChanged]);
    assert_eq!(review.prompt_tx, prompt_tx);
    let summary = tally(&[asked]);
    assert!(
        summary.contains("wrong work: 1 of 1 found unfinished"),
        "{summary}"
    );
    assert!(
        summary.contains("right work: 0 of 0 found unfinished"),
        "{summary}"
    );
}

/// A writer that answers once, calling the criteria tool with `items`.
fn writer_of(items: &serde_json::Value) -> Arc<dyn ModelProvider + Send + Sync> {
    let reply = aura_model::MockResponse::tool_use(
        "toolu_1",
        criteria::CRITERIA_TOOL,
        json!({"items": items, "strict": true}),
    );
    Arc::new(aura_model::MockProvider::new().with_response(reply))
}

/// The two criteria the writer lists for [`session`]'s request.
fn two_criteria() -> serde_json::Value {
    json!([
        {"text": "f() returns 2", "kind": "behaviour"},
        {"text": "f(-1) raises ValueError", "kind": "error"},
    ])
}

#[tokio::test]
async fn each_request_gets_its_criteria_written_once_and_kept() {
    let dir = TempDir::new().unwrap();
    let Some(ready) = kept_attempt(&dir).await else {
        return;
    };

    // Two attempts at one request: its criteria are written once, and
    // kept for the next replay.
    let writer = writer_of(&two_criteria());
    let mut written = Written::in_data_dir(dir.path());
    let twice = vec![ready.clone(), ready.clone()];
    let (held, cost_usd) = hold_to_criteria(twice, &mut written, SONNET, (&writer, None))
        .await
        .unwrap();
    assert_eq!(held.len(), 2);
    assert!(cost_usd > 0.0);
    let criteria = held[0].criteria.clone().unwrap();
    assert_eq!((criteria.items.len(), criteria.strict), (2, true));
    assert_eq!(
        held[0].why,
        [
            ("hard".to_string(), Why::Hard.describe().to_string()),
            ("criteria".to_string(), Why::Criteria.describe().to_string()),
        ]
    );
    let kept = Written::in_data_dir(dir.path());
    assert_eq!(kept.get(SONNET, &ready.task), Some(criteria));
    assert_eq!(
        kept.get(OPUS, &ready.task),
        None,
        "another writer's are its own"
    );

    // A request the writer gives no criteria for is left out.
    let silent = writer_of(&json!([]));
    let mut none = Written::in_data_dir(&dir.path().join("elsewhere"));
    let (held, _) = hold_to_criteria(vec![ready], &mut none, SONNET, (&silent, None))
        .await
        .unwrap();
    assert!(held.is_empty());
}

#[tokio::test]
async fn held_to_criteria_the_review_goes_through_them() {
    let dir = TempDir::new().unwrap();
    let Some(ready) = kept_attempt(&dir).await else {
        return;
    };
    let writer = writer_of(&two_criteria());
    let mut written = Written::in_data_dir(dir.path());
    let (held, _) = hold_to_criteria(vec![ready], &mut written, SONNET, (&writer, None))
        .await
        .unwrap();

    // The reviewer is shown them, and its line for each is logged.
    let why = held[0].why_told();
    let shown = turn::review_prompt(&held[0].standing(&why));
    assert!(
        shown.contains("2. [error] f(-1) raises ValueError"),
        "{shown}"
    );
    let reply = "CRITERION 1: MET by test_f\nCRITERION 2: UNMET, nothing raises\n\
                 1. Negative input is not rejected.\nVERDICT: MODEL";
    let reviewer = reviewer(&dir, OPUS, reply);
    let asked: Asked = reviewer.ask(held[0].clone()).await;
    let shown = asked.row();
    assert!(
        shown.contains("unfinished") && shown.contains(", criteria 1/2 met, 1 unmet"),
        "{shown}"
    );
    let written = reviewer.log.read().unwrap().lines;
    let [Line::Review(review)] = &written[..] else {
        panic!("{written:?}");
    };
    assert_eq!(review.variant, "plain+criteria");
    assert_eq!(review.why, ["hard", "criteria"]);
    assert_eq!(
        review
            .criteria
            .iter()
            .map(|checked| checked.status)
            .collect::<Vec<_>>(),
        [Some(criteria::Status::Met), Some(criteria::Status::Unmet)]
    );
}

#[tokio::test]
async fn an_attempt_is_written_again_when_its_run_shows_more_than_its_line() {
    let dir = TempDir::new().unwrap();
    let Some(ready) = kept_attempt(&dir).await else {
        return;
    };
    // Another attempt whose line already holds what its run shows, which
    // is what the run of `ready` is now shown to have done.
    let other = Attempt {
        evidence: ready.attempt.evidence.clone(),
        ..attempt(AgentId::generate(), 1, SONNET, Some(Difficulty::Hard))
    };
    let other_ready = Prepared {
        attempt: other.clone(),
        ..ready.clone()
    };
    let logged = [
        Attempt {
            evidence: Evidence::default(),
            ..ready.attempt.clone()
        },
        other,
    ];
    let copy = CopyAt {
        root: dir.path().to_path_buf(),
        git_dir: None,
        baseline: String::new(),
    };
    let mut lines: Vec<Line> = logged.iter().cloned().map(Line::Attempt).collect();
    lines.extend(logged.iter().map(|attempt| {
        Line::Truth(Truth {
            copy: Some(copy.clone()),
            ..truth(attempt, false)
        })
    }));
    let (picks, _) = pick(&lines, &Filter::default(), &HashSet::new(), |_| true);
    assert_eq!(picks.len(), 2);

    // In whatever order they come, each is held against its own line.
    let log = AttemptLog::in_data_dir(dir.path());
    rewrite(&log, &[other_ready, ready.clone()], &picks).unwrap();
    let written = log.read().unwrap().lines;
    let [Line::Attempt(again)] = &written[..] else {
        panic!("{written:?}");
    };
    assert_eq!(again.prompt_tx, ready.attempt.prompt_tx);
    assert_eq!(again.evidence, ready.attempt.evidence);
}
