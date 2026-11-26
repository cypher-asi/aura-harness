use super::*;
use aura_agent_trace::bundle::{RunBundle, RunSource, STEPS_FILE};
use aura_agent_trace::run_log;
use aura_agent_trace::RunMeta;
use aura_core_types::trace::{
    Actor, ModelAction, Observation, PolicyRef, Step, StepAction, StepLinks, ToolCallAction,
    ToolIntent,
};
use aura_ui::events::LogStatus;
use serde_json::json;

fn step(run: &str, seq: u64, actor: Actor, action: StepAction) -> Step {
    Step {
        run_id: run.into(),
        seq,
        parent: None,
        turn: 1,
        iteration: Some(0),
        at_ms: 1_000 + seq,
        actor,
        observation: Observation::None,
        action,
        policy: PolicyRef::default(),
        links: StepLinks::default(),
        usage: None,
        duration_ms: None,
    }
}

fn save(home: &Path, meta: &RunMeta, steps: &[Step]) -> RunBundle {
    let bundle = RunBundle::create(home, meta).unwrap();
    let lines: Vec<String> = steps
        .iter()
        .map(|s| serde_json::to_string(s).unwrap())
        .collect();
    std::fs::write(bundle.path(STEPS_FILE), lines.join("\n") + "\n").unwrap();
    bundle
}

/// A prompt to run `pid` and its model's `task` call with `input`.
fn delegating(pid: &str, input: serde_json::Value) -> Vec<Step> {
    vec![
        step(
            pid,
            1,
            Actor::User,
            StepAction::UserPrompt {
                text: "delegate it".into(),
            },
        ),
        step(
            pid,
            2,
            Actor::Model,
            StepAction::Model(ModelAction {
                thinking_text: "a subagent can count".into(),
                thinking_chars: 20,
                tool_calls: vec![ToolCallAction {
                    tool_use_id: "t1".into(),
                    name: "task".into(),
                    intent: ToolIntent::classify("task", &input),
                    input,
                }],
                stop_reason: "tool_use".into(),
                ..ModelAction::default()
            }),
        ),
    ]
}

#[test]
fn a_run_and_its_subagent_are_laid_out_with_their_files() {
    let home = tempfile::tempdir().unwrap();
    let parent = RunMeta::new("claude/claude-sonnet-5-5", RunSource::Test);
    let pid = parent.run_id.clone();
    let input = json!({"prompt": "count files"});
    let bundle = save(home.path(), &parent, &delegating(&pid, input));
    let mut child = RunMeta::new("m", RunSource::Test);
    child.title = "count files".into();
    child.spawned_by = Some(pid.clone());
    save(home.path(), &child, &[]);

    let log = run_log::of_bundle(home.path(), &bundle).unwrap();
    let view = view("ab", &log, &bundle.path(run_log::LOG_FILE));
    assert_eq!(view.runs.len(), 2);
    let root = view.root().unwrap();
    assert_eq!(root.depth, 0);
    assert_eq!(view.runs[1].depth, 1);
    assert!(root
        .files
        .iter()
        .any(|(label, path)| label == "log" && path.ends_with("log.json")));
    let call = &root.turns[0].steps[1];
    assert_eq!(call.spawned, vec![1], "the task call names its subagent");
    assert!(call
        .sections
        .iter()
        .any(|s| s.title == "Thinking" && s.body == "a subagent can count"));
    assert_eq!(root.status, LogStatus::Running, "the turn has not ended");
    assert!(view.json.contains("\"schema\": \"aura.run_log/v1\""));
}

/// The `list_files` call `t1` with `input`, which took 6 s.
fn list_call(id: &str, input: serde_json::Value) -> Step {
    let mut call = step(
        id,
        2,
        Actor::Model,
        StepAction::Model(ModelAction {
            tool_calls: vec![ToolCallAction {
                tool_use_id: "t1".into(),
                name: "list_files".into(),
                intent: ToolIntent::classify("list_files", &input),
                input,
            }],
            stop_reason: "tool_use".into(),
            ..ModelAction::default()
        }),
    );
    call.duration_ms = Some(6_000);
    call
}

/// The result of `t1`, `listing`, which took 19 ms.
fn list_result(id: &str, listing: &str) -> Step {
    let mut result = step(
        id,
        3,
        Actor::Environment,
        StepAction::ToolResult {
            tool_use_id: "t1".into(),
            tool: "list_files".into(),
            is_error: false,
            source: "executor".into(),
            content_chars: listing.len(),
            excerpt: listing.into(),
            content: None,
            files: Vec::new(),
        },
    );
    result.parent = Some(2);
    result.duration_ms = Some(19);
    result
}

#[test]
fn a_tool_result_reads_as_the_call_it_answers_and_what_came_of_it() {
    let home = tempfile::tempdir().unwrap();
    let mut meta = RunMeta::new("m", RunSource::Test);
    meta.title = "list the crates".into();
    let id = meta.run_id.clone();
    let input = json!({"path": "crates"});
    let listing = "dir\t4096\tagent\ndir\t4096\tui";
    let (call, result) = (list_call(&id, input), list_result(&id, listing));
    let prompt = StepAction::UserPrompt {
        text: "list the crates".into(),
    };
    let bundle = save(
        home.path(),
        &meta,
        &[step(&id, 1, Actor::User, prompt), call, result],
    );

    let log = run_log::of_bundle(home.path(), &bundle).unwrap();
    let view = view("ab", &log, &bundle.path(run_log::LOG_FILE));
    let turn = &view.root().unwrap().turns[0];
    assert_eq!(turn.prompt, "", "the run's title says it");
    let (call, result) = (&turn.steps[1], &turn.steps[2]);
    assert_eq!(call.label, "list_files crates");
    assert_eq!((call.took.as_str(), call.parent), ("6s", None));
    assert!(call.tokens.ends_with(" tok"), "{:?}", call.tokens);
    assert_eq!(result.label, "list_files crates");
    assert_eq!(result.summary, "2 entries");
    assert_eq!((result.took.as_str(), result.parent), ("19ms", Some(2)));
    assert!(
        result.sections.iter().any(|s| s.body == listing),
        "kept whole"
    );
}
