use super::*;
use aura_agent_gate::attempt_log::evaluate;
use aura_agent_gate::evidence::{flags, Flag};
use aura_agent_kernel::RecordObserver;
use aura_core_types::{
    BenchSource, CompareArm, CompareCopy, CompareStatus, Difficulty, PromptCategory, PromptPayload,
    Route, RouteReason, SystemKind, Transaction,
};
use aura_model::ToolResultContent;
use aura_store_db::{RocksStore, WriteStore};
use serde_json::json;
use std::path::PathBuf;
use tempfile::TempDir;

use crate::session_checkpoint::SessionCheckpoint;

const SONNET: &str = "claude/claude-sonnet-5-5";
const OPUS: &str = "claude/claude-opus-5-5";
const TASK: &str = "Make ensure_type() follow the PR.";
const TAKEOVER: &str = "Harness notice:\nThe work above was done on Claude Sonnet 5.5 and ended with its tests failing.";

fn entry(seq: u64, tx: Transaction) -> RecordEntry {
    RecordEntry::builder(seq, tx).build()
}

fn prompt(agent: AgentId, text: &str, model: &str, reason: RouteReason) -> Transaction {
    let mut payload = PromptPayload::new(text);
    payload.category = PromptCategory::MultiFileChange;
    payload.route = Route::turn(model, reason);
    if reason == RouteReason::Compare {
        payload.difficulty = Some(Difficulty::Hard);
        payload.led = Some(false);
    }
    Transaction::user_prompt_with(agent, &payload)
}

fn ended(prompt_tx: Hash, model: &str, signal: OutcomeSignal) -> OutcomePayload {
    OutcomePayload {
        prompt_tx,
        category: PromptCategory::MultiFileChange,
        model: model.to_string(),
        success: signal.success(),
        signal,
        cost_usd: 0.1,
        subtask_cost_usd: 0.0,
        latency_ms: 48_000,
    }
}

fn outcome(agent: AgentId, prompt_tx: Hash, model: &str, signal: OutcomeSignal) -> Transaction {
    Transaction::outcome(agent, &ended(prompt_tx, model, signal))
}

fn ran(id: &str, command: &str, output: &str, is_error: bool) -> [Message; 2] {
    [
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: "run_command".to_string(),
                input: json!({"command": command}),
            }],
        ),
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: ToolResultContent::Text(output.to_string()),
                is_error,
            }],
        ),
    ]
}

/// A Gate run of a benchmark task: Sonnet's attempt fails its own check
/// and is judged at fault, Opus takes over, is asked to check its change,
/// pipes the check and is let stand; the task's check then fails.
fn run(agent: AgentId) -> (Vec<RecordEntry>, Vec<Message>) {
    let first = prompt(agent, TASK, SONNET, RouteReason::Compare);
    let moved = prompt(agent, TAKEOVER, OPUS, RouteReason::Escalate);
    let nudge = Transaction::user_prompt_with(
        agent,
        &PromptPayload::new(super::super::gate_followup::nudge_prompt()),
    );
    let (p1, p2) = (first.hash, moved.hash);
    let entries = vec![
        entry(1, first),
        entry(2, outcome(agent, p1, SONNET, OutcomeSignal::ChecksFailed)),
        entry(3, outcome(agent, p1, SONNET, OutcomeSignal::JudgedFail)),
        entry(4, moved),
        entry(5, nudge),
        entry(6, outcome(agent, p2, OPUS, OutcomeSignal::ChecksPassed)),
        // The task's own check, recorded for the same prompt.
        entry(7, outcome(agent, p2, OPUS, OutcomeSignal::ChecksFailed)),
    ];
    let mut messages = vec![Message::user(TASK)];
    messages.extend(ran(
        "1",
        "pytest -q",
        "exit_code: 1\n2 failed, 7 passed",
        true,
    ));
    messages.push(Message::assistant("Two tests still fail."));
    messages.push(Message::user(TAKEOVER));
    messages.push(Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: "2".to_string(),
            name: "edit_file".to_string(),
            input: json!({"path": "lib/config/manager.py"}),
        }],
    ));
    messages.push(Message::tool_results(vec![(
        "2".to_string(),
        ToolResultContent::Text("ok".to_string()),
        false,
    )]));
    messages.push(Message::assistant("Rewrote ensure_type()."));
    messages.push(Message::user(super::super::gate_followup::nudge_prompt()));
    messages.extend(ran(
        "3",
        "pytest -q 2>&1 | grep -E 'passed|failed|Error' | head -20",
        "TypeError: _os_stat() got an unexpected keyword argument\n9 passed",
        false,
    ));
    messages.push(Message::assistant(
        "I rewrote `ensure_type()`, but it isn't verified.",
    ));
    (entries, messages)
}

#[test]
fn a_runs_attempts_are_rebuilt_from_its_record_and_session() {
    let agent = AgentId::generate();
    let (entries, messages) = run(agent);
    let attempts = attempts_of(&entries, &messages, true).unwrap();
    assert_eq!(attempts.len(), 2);

    let first = &attempts[0];
    assert_eq!(first.prompt_tx, entries[0].tx.hash.to_hex());
    assert_eq!(
        (first.model.as_str(), first.signal),
        (SONNET, OutcomeSignal::ChecksFailed)
    );
    assert_eq!(
        first.success,
        Some(false),
        "what Gate made of it, not the judge's entry"
    );
    assert_eq!(first.evidence.checks_run, 1);
    assert!(!first.evidence.last_check().unwrap().passed);
    assert_eq!(first.evidence.reply, "Two tests still fail.");

    let second = &attempts[1];
    assert_eq!(second.prompt_tx, entries[3].tx.hash.to_hex());
    assert_eq!(
        (second.model.as_str(), second.signal),
        (OPUS, OutcomeSignal::ChecksPassed)
    );
    assert_eq!(second.evidence.nudges, 1);
    assert_eq!(second.evidence.files_edited, ["lib/config/manager.py"]);
    assert_eq!(
        (
            second.difficulty,
            second.led,
            second.routed,
            second.backfilled
        ),
        (Some(Difficulty::Hard), Some(false), true, true),
        "the label of the prompt the turn began with"
    );
    assert!(flags(&second.evidence, second.category).contains(&Flag::MaskedCheckWithFailures));
}

#[test]
fn a_session_that_does_not_hold_the_records_prompts_is_left_out() {
    let agent = AgentId::generate();
    let (entries, mut messages) = run(agent);
    // As after compaction replaced the start of the conversation.
    messages.drain(..5);
    assert_eq!(attempts_of(&entries, &messages, true), None);
    assert_eq!(attempts_of(&[], &messages, true), None);
}

fn comparison(run: CompareRun) -> ComparePayload {
    ComparePayload {
        system_kind: SystemKind::Compare,
        task: "benchmark".to_string(),
        plan: None,
        arm_a: ArmSpec::Gate { delegate: true },
        arm_b: ArmSpec::Model {
            model: OPUS.to_string(),
        },
        against: OPUS.to_string(),
        prompts: vec![TASK.to_string()],
        source: BenchSource::Prompt,
        repeats: 1,
        check: None,
        budget_usd: None,
        requested_by: AgentId::generate(),
        apply_root: PathBuf::from("/work/repo"),
        started_ms: 1_000,
        runs: vec![run],
        judgments: Vec::new(),
        notes: Vec::new(),
        status: CompareStatus::Picked(CompareArm::A),
    }
}

fn compare_run(agent: AgentId, prompt_tx: Hash) -> CompareRun {
    CompareRun {
        name: "a-4".to_string(),
        arm: CompareArm::A,
        agent_id: agent,
        prompt_tx: Some(prompt_tx),
        model: SONNET.to_string(),
        copy: CompareCopy {
            root: PathBuf::from("/tmp/a-4"),
            workdir: PathBuf::from("/tmp/a-4/app"),
            git_dir: None,
            baseline: "abc".to_string(),
        },
        wall_ms: 48_000,
        cost_usd: 0.1,
        tokens: Vec::new(),
        turns: Vec::new(),
        plan_usage: Vec::new(),
        check_passed: Some(false),
        files_changed: 2,
        error: None,
        task: Some("ansible-d33bedc4".to_string()),
    }
}

#[tokio::test]
async fn recorded_comparisons_are_caught_up_on_once() {
    let dir = TempDir::new().unwrap();
    let store = RocksStore::open(dir.path().join("db"), false).unwrap();
    let (host, agent) = (AgentId::generate(), AgentId::generate());
    let (entries, messages) = run(agent);
    for e in &entries {
        store.append_entry_direct(agent, e.seq, e).unwrap();
    }
    let payload = comparison(compare_run(agent, entries[0].tx.hash));
    store
        .append_entry_direct(host, 1, &entry(1, Transaction::compare(host, &payload)))
        .unwrap();
    let sessions = Checkpoints::new(dir.path());
    let saved = SessionCheckpoint::new(5, 7, messages, Vec::new(), Default::default());
    sessions.save(agent, &saved).await.unwrap();

    let log = AttemptLog::in_data_dir(dir.path());
    let hosts = [(host, "Compare #22".to_string())];
    let caught = catch_up(&store, &hosts, &sessions, &log);
    assert_eq!(
        caught,
        CaughtUp {
            runs: 1,
            attempts: 2,
            truths: 1,
            skipped: 0
        }
    );
    assert_eq!(
        catch_up(&store, &hosts, &sessions, &log),
        CaughtUp::default()
    );

    let report = evaluate(&log.read().unwrap().lines);
    assert_eq!((report.attempts, report.routed), (2, 2));
    assert_eq!(report.verdicts.false_passes, 1);
    let missed = report.missed().next().unwrap();
    assert_eq!(missed.run, "Compare #22 a-4");
    assert_eq!(missed.task, "ansible-d33bedc4");
    assert_eq!(missed.model, OPUS);

    // A run with no saved session is left out, and said so.
    let other = AgentId::generate();
    let (entries, _) = run(other);
    for e in &entries {
        store.append_entry_direct(other, e.seq, e).unwrap();
    }
    let payload = comparison(compare_run(other, entries[0].tx.hash));
    store
        .append_entry_direct(host, 2, &entry(2, Transaction::compare(host, &payload)))
        .unwrap();
    let caught = catch_up(&store, &hosts, &sessions, &log);
    assert_eq!((caught.runs, caught.attempts, caught.skipped), (1, 0, 1));
}

#[test]
fn attempts_and_the_users_verdicts_are_logged_as_they_happen() {
    let dir = TempDir::new().unwrap();
    let live = GateLive::default();
    let agent = AgentId::generate();
    let asked = prompt(agent, TASK, SONNET, RouteReason::Default);
    let done = ended(asked.hash, SONNET, OutcomeSignal::ChecksPassed);
    let terms = Terms {
        difficulty: Some(Difficulty::Hard),
        led: Some(false),
        routed: true,
    };
    let rewound = entry(3, outcome(agent, asked.hash, SONNET, OutcomeSignal::Rewind));

    // Nothing is written until a log is set.
    live.log_attempt(agent, &done, terms, None);
    live.on_entry(&rewound);
    assert!(live.attempt_log().is_none());

    live.log_attempts_to(AttemptLog::in_data_dir(dir.path()));
    let (_, messages) = run(agent);
    live.log_attempt(agent, &done, terms, Some(Evidence::of(&messages[4..])));
    live.on_entry(&entry(2, Transaction::outcome(agent, &done)));
    live.on_entry(&rewound);
    live.log_truth(
        agent,
        asked.hash,
        false,
        RunOf {
            compare: "Compare #22",
            run: "a-4",
            task: Some("ansible-d33bedc4"),
        },
    );

    let lines = live.attempt_log().unwrap().read().unwrap().lines;
    assert_eq!(lines.len(), 3, "the attempt's own outcome is not a truth");
    let Line::Attempt(attempt) = &lines[0] else {
        panic!("{:?}", lines[0]);
    };
    assert_eq!(attempt.agent, agent.to_hex());
    assert_eq!(attempt.prompt_tx, asked.hash.to_hex());
    assert!(!attempt.backfilled);
    assert_eq!(attempt.evidence.nudges, 1);
    let Line::Truth(by_user) = &lines[1] else {
        panic!("{:?}", lines[1]);
    };
    assert_eq!(
        (by_user.source, by_user.passed),
        (TruthSource::Rewind, false)
    );
    assert_eq!(by_user.prompt_tx, asked.hash.to_hex());
    let Line::Truth(by_task) = &lines[2] else {
        panic!("{:?}", lines[2]);
    };
    assert_eq!(by_task.source, TruthSource::TaskCheck);
    assert_eq!(by_task.run.as_deref(), Some("a-4"));
    assert_eq!(evaluate(&lines).verdicts.false_passes, 1);
}
