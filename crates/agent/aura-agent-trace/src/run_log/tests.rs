use super::*;
use crate::bundle::{RunMeta, RunSource, STEPS_FILE};
use aura_core_types::trace::{
    Actor, DecisionSource, HarnessAction, ModelAction, Observation, PolicyKind, PolicyRef, Step,
    StepAction, StepLinks, ToolCallAction, ToolIntent,
};
use serde_json::json;

fn step(run: &str, seq: u64, turn: u32, actor: Actor, action: StepAction) -> Step {
    Step {
        run_id: run.into(),
        seq,
        parent: None,
        turn,
        iteration: Some(0),
        at_ms: 1_000 + seq * 10,
        actor,
        observation: Observation::None,
        action,
        policy: PolicyRef {
            kind: PolicyKind::Model,
            id: "claude/claude-sonnet-5-5".into(),
            ..PolicyRef::default()
        },
        links: StepLinks::default(),
        usage: None,
        duration_ms: None,
    }
}

fn prompt(run: &str, seq: u64, turn: u32, text: &str) -> Step {
    step(
        run,
        seq,
        turn,
        Actor::User,
        StepAction::UserPrompt { text: text.into() },
    )
}

fn call(run: &str, seq: u64, turn: u32, id: &str, tool: &str, input: Value) -> Step {
    let mut s = step(
        run,
        seq,
        turn,
        Actor::Model,
        StepAction::Model(ModelAction {
            thinking_chars: 12,
            thinking_text: "check the file".into(),
            text: "Reading it.".into(),
            tool_calls: vec![ToolCallAction {
                tool_use_id: id.into(),
                name: tool.into(),
                intent: ToolIntent::classify(tool, &input),
                input,
            }],
            stop_reason: "tool_use".into(),
            ..ModelAction::default()
        }),
    );
    s.usage = Some(TokenUsage {
        input: 100,
        output: 20,
        ..TokenUsage::default()
    });
    s
}

fn end(run: &str, seq: u64, turn: u32) -> Step {
    step(
        run,
        seq,
        turn,
        Actor::Environment,
        StepAction::TurnEnd {
            stop_reason: "end_turn".into(),
            iterations: 1,
            input_tokens: 100,
            output_tokens: 20,
            files_changed: 0,
            error: None,
        },
    )
}

fn save(home: &Path, meta: &RunMeta, steps: &[Step]) -> RunBundle {
    let bundle = RunBundle::create(home, meta).unwrap();
    bundle::append_jsonl(&bundle.path(STEPS_FILE), steps).unwrap();
    bundle
}

/// A nudge from steering, with what fired it.
fn steering_nudge(run: &str, seq: u64, turn: u32) -> Step {
    let mut nudge = step(
        run,
        seq,
        turn,
        Actor::Harness {
            source: DecisionSource::Steering,
        },
        StepAction::Harness {
            name: "implement_now".into(),
            action: HarnessAction::Nudge {
                kind: "implement".into(),
            },
            alternatives: Vec::new(),
        },
    );
    nudge.observation = Observation::Harness {
        features: json!({"reads": 9}),
        trigger: json!({"threshold": 8}),
    };
    nudge
}

/// A stored two-turn run: the second turn's call kept no thinking text
/// and was followed by a steering nudge.
fn two_turn_run() -> (tempfile::TempDir, RunBundle, RunLog) {
    let home = tempfile::tempdir().unwrap();
    let meta = RunMeta::new("claude/claude-sonnet-5-5", RunSource::Test);
    let run = meta.run_id.clone();
    let mut old = call(&run, 5, 2, "t2", "read_file", json!({"path": "b.rs"}));
    if let StepAction::Model(action) = &mut old.action {
        action.thinking_text.clear();
    }
    let bundle = save(
        home.path(),
        &meta,
        &[
            prompt(&run, 1, 1, "fix the bug"),
            call(&run, 2, 1, "t1", "read_file", json!({"path": "a.rs"})),
            end(&run, 3, 1),
            prompt(&run, 4, 2, "and the other"),
            old,
            steering_nudge(&run, 6, 2),
            end(&run, 7, 2),
        ],
    );
    let log = of_bundle(home.path(), &bundle).unwrap();
    (home, bundle, log)
}

#[test]
fn groups_steps_by_turn() {
    let (_home, _bundle, log) = two_turn_run();
    assert_eq!(log.schema, LOG_SCHEMA);
    assert_eq!(log.turns.len(), 2);
    let first = &log.turns[0];
    assert_eq!(first.prompt.as_deref(), Some("fix the bug"));
    assert_eq!(first.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!((first.model_calls, first.tool_calls), (1, 1));
}

#[test]
fn model_calls_carry_thinking_or_say_it_was_omitted() {
    let (_home, _bundle, log) = two_turn_run();
    match &log.turns[0].steps[1].event {
        LogEvent::ModelCall {
            thinking,
            thinking_omitted,
            ..
        } => {
            assert_eq!(thinking.as_deref(), Some("check the file"));
            assert!(!thinking_omitted);
        }
        other => panic!("not a model call: {other:?}"),
    }
    match &log.turns[1].steps[1].event {
        LogEvent::ModelCall {
            thinking,
            thinking_omitted,
            ..
        } => {
            assert!(thinking.is_none(), "an older run kept no thinking text");
            assert!(thinking_omitted);
        }
        other => panic!("not a model call: {other:?}"),
    }
}

#[test]
fn decisions_carry_their_source_and_trigger() {
    let (_home, _bundle, log) = two_turn_run();
    match &log.turns[1].steps[2].event {
        LogEvent::Decision {
            source, trigger, ..
        } => {
            assert_eq!(source, "steering");
            assert_eq!(trigger["threshold"], 8);
        }
        other => panic!("not a decision: {other:?}"),
    }
    assert_eq!(log.turns[1].decisions, 1);
}

#[test]
fn totals_and_files_cover_the_run() {
    let (_home, bundle, log) = two_turn_run();
    assert_eq!(log.totals.model_calls, 2);
    assert_eq!(log.totals.usage.input, 200);
    assert!(log.totals.cost_usd.is_some_and(|c| c > 0.0));
    assert_eq!(log.files.bundle, bundle.dir().display().to_string());
    assert!(log.files.log.ends_with(LOG_FILE));
}

#[test]
fn written_log_is_pretty_and_reads_back() {
    let (_home, bundle, log) = two_turn_run();
    let path = write(bundle.dir(), &log).unwrap();
    let written = std::fs::read_to_string(path).unwrap();
    assert!(written.starts_with("{\n  \"schema\""), "pretty-printed");
    let back: RunLog = serde_json::from_str(&written).unwrap();
    assert_eq!(back, log);
}

#[test]
fn an_agent_without_a_run_groups_the_runs_that_belong_to_it() {
    let home = tempfile::tempdir().unwrap();
    let logs: Vec<RunLog> = (0..2)
        .map(|n| {
            let mut meta = RunMeta::new("m", RunSource::Bench);
            meta.title = format!("a-{n}");
            let id = meta.run_id.clone();
            let bundle = save(
                home.path(),
                &meta,
                &[
                    prompt(&id, 1, 1, "write it"),
                    call(&id, 2, 1, "t1", "write_file", json!({"path": "a"})),
                    end(&id, 3, 1),
                ],
            );
            of_bundle(home.path(), &bundle).unwrap()
        })
        .collect();
    let again = logs[0].clone();
    let dir = home.path().join("logs/abc");
    let mut log = group("abc", "Compare #1", &dir, logs);
    adopt(&mut log, vec![again]);

    assert_eq!(log.schema, LOG_SCHEMA);
    assert_eq!(log.run.title, "Compare #1");
    assert_eq!(log.run.source, "agent");
    assert!(log.turns.is_empty());
    assert_eq!(log.files.log, dir.join(LOG_FILE).display().to_string());
    let titles: Vec<&str> = log.subagents.iter().map(|r| r.run.title.as_str()).collect();
    assert_eq!(titles, ["a-0", "a-1"], "each run once");
    assert!(log.subagents.iter().all(|r| r.schema.is_empty()));
    assert_eq!(log.totals.subagents, 2);
    let each = log.subagents[0].totals.cost_usd_with_subagents.unwrap();
    assert!((log.totals.cost_usd_with_subagents.unwrap() - 2.0 * each).abs() < 1e-9);
}

#[test]
fn nests_spawned_runs_under_the_call_that_started_them() {
    let home = tempfile::tempdir().unwrap();
    let parent = RunMeta::new("m", RunSource::Test);
    let pid = parent.run_id.clone();
    let task = json!({"prompt": "count files"});
    let bundle = save(
        home.path(),
        &parent,
        &[
            prompt(&pid, 1, 1, "delegate"),
            call(&pid, 2, 1, "t1", "task", task),
            end(&pid, 3, 1),
        ],
    );
    let mut child = RunMeta::new("m", RunSource::Test);
    child.title = "count files".into();
    child.spawned_by = Some(pid.clone());
    child.started_at_ms = parent.started_at_ms + 1;
    let cid = child.run_id.clone();
    let child_bundle = save(
        home.path(),
        &child,
        &[prompt(&cid, 1, 1, "count files"), end(&cid, 2, 1)],
    );
    let mut grandchild = RunMeta::new("m", RunSource::Test);
    grandchild.spawned_by = Some(cid.clone());
    save(
        home.path(),
        &grandchild,
        &[prompt(&grandchild.run_id, 1, 1, "x")],
    );

    let log = of_bundle(home.path(), &bundle).unwrap();
    assert_eq!(log.subagents.len(), 1);
    assert_eq!(log.totals.subagents, 2, "the grandchild counts too");
    let sub = &log.subagents[0];
    assert_eq!(sub.run.run_id, cid);
    let spawn = sub.run.spawned_by.as_ref().unwrap();
    assert_eq!(spawn.tool_use_id.as_deref(), Some("t1"));
    assert_eq!((spawn.turn, spawn.step), (Some(1), Some(2)));
    assert_eq!(log.turns[0].subagents, vec![cid.clone()]);
    assert!(log.find(&grandchild.run_id).is_some());
    match &log.turns[0].steps[1].event {
        LogEvent::ModelCall { tool_calls, .. } => {
            assert_eq!(tool_calls[0].child_run.as_deref(), Some(cid.as_str()));
        }
        other => panic!("not a model call: {other:?}"),
    }

    let only_child = of_bundle(home.path(), &child_bundle).unwrap();
    assert_eq!(only_child.run.run_id, cid);
    assert_eq!(only_child.subagents.len(), 1, "only what it spawned");
    assert_eq!(home_of(child_bundle.dir()), Some(home.path()));
}

#[test]
fn secrets_are_redacted() {
    let home = tempfile::tempdir().unwrap();
    let meta = RunMeta::new("m", RunSource::Test);
    let run = meta.run_id.clone();
    let bundle = save(
        home.path(),
        &meta,
        &[prompt(&run, 1, 1, "use api_key=sk-abcdefghijklmnop please")],
    );
    let log = of_bundle(home.path(), &bundle).unwrap();
    assert!(log.redactions > 0);
    let text = log.turns[0].prompt.as_deref().unwrap();
    assert!(text.contains("[REDACTED]") && !text.contains("sk-abcdefghijklmnop"));
}
