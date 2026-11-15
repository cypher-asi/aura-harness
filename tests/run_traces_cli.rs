//! `aura runs` / `aura dataset` against a recorded run in a temp
//! `AURA_HOME`.

use std::path::Path;
use std::process::Command;

use aura_agent::{AgentLoopEvent, AgentLoopResult, DebugEvent};
use aura_agent_trace::{RunMeta, RunRecorder, RunSource};
use aura_config::TraceLevel;
use aura_core_types::trace::{
    DecisionSource, HarnessAction, HarnessDecision, ModelAction, ModelExchange, TokenUsage,
    ToolCallAction, ToolIntent,
};
use serde_json::json;

fn aura(home: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_aura"))
        .args(args)
        .env("AURA_HOME", home)
        .output()
        .expect("run aura");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn record_run(home: &Path, workspace: &Path) -> String {
    let mut meta = RunMeta::new("test-model", RunSource::Test);
    meta.workspace = workspace.display().to_string();
    let rec = RunRecorder::start(home, meta, TraceLevel::Full)
        .expect("start")
        .expect("enabled");
    rec.begin_turn("add a greeting");
    rec.on_event(&AgentLoopEvent::Debug(DebugEvent::Decision {
        timestamp: chrono::Utc::now(),
        decision: Box::new(
            HarnessDecision::new(
                DecisionSource::Steering,
                "implement_now",
                HarnessAction::Nudge {
                    kind: "implement_now".into(),
                },
            )
            .at_iteration(0),
        ),
        task_id: None,
    }));
    let input = json!({"path": "hello.txt", "content": "hi"});
    rec.on_event(&AgentLoopEvent::Debug(DebugEvent::ModelExchange {
        timestamp: chrono::Utc::now(),
        exchange: Box::new(ModelExchange {
            iteration: 0,
            model: "test-model".into(),
            observation: None,
            action: ModelAction {
                tool_calls: vec![ToolCallAction {
                    tool_use_id: "t1".into(),
                    name: "write_file".into(),
                    intent: ToolIntent::classify("write_file", &input),
                    input,
                }],
                stop_reason: "tool_use".into(),
                ..ModelAction::default()
            },
            usage: TokenUsage {
                input: 1200,
                output: 80,
                cache_write: 0,
                cache_read: 600,
            },
            latency_ms: 900,
            provider_request_id: None,
        }),
        task_id: None,
    }));
    rec.on_event(&AgentLoopEvent::ToolResult {
        tool_use_id: "t1".into(),
        tool_name: "write_file".into(),
        content: "wrote hello.txt".into(),
        is_error: false,
        image: None,
    });
    rec.end_turn(&AgentLoopResult::default());
    rec.finish();
    rec.run_id()
}

#[test]
fn runs_and_dataset_commands_work_on_a_recorded_run() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let run_id = record_run(home.path(), workspace.path());
    let prefix = &run_id[..10];

    let (ok, out) = aura(home.path(), &["runs", "list"]);
    assert!(ok, "{out}");
    assert!(out.contains("test-model"), "{out}");
    assert!(out.contains("add a greeting"), "{out}");

    let (ok, out) = aura(home.path(), &["runs", "show", prefix, "--steps"]);
    assert!(ok, "{out}");
    assert!(out.contains("steering/implement_now"), "{out}");
    assert!(out.contains("model → write_file"), "{out}");

    let (ok, out) = aura(home.path(), &["runs", "verify", prefix, "--cmd", "true"]);
    assert!(ok, "{out}");
    assert!(out.contains("passed"), "{out}");

    let (ok, out) = aura(home.path(), &["runs", "label", prefix, "-1", "--step", "3"]);
    assert!(ok, "{out}");

    let (ok, out) = aura(home.path(), &["runs", "show", prefix]);
    assert!(ok, "{out}");
    assert!(out.contains("outcome   +1.00"), "{out}");

    let (ok, out) = aura(home.path(), &["runs", "diff", prefix, prefix]);
    assert!(ok, "{out}");
    assert!(!out.contains("regression("), "{out}");

    let out_dir = home.path().join("sft");
    let (ok, out) = aura(
        home.path(),
        &[
            "dataset",
            "build",
            "--format",
            "sft",
            "--filter",
            "eval=include,eval_fraction=0",
            "--out",
            out_dir.to_str().unwrap(),
        ],
    );
    assert!(ok, "{out}");
    assert!(out.contains("1 of 1 runs"), "{out}");
    let train = std::fs::read_to_string(out_dir.join("train.jsonl")).unwrap();
    assert!(train.contains("write_file"));
    assert!(out_dir.join("dataset_card.json").is_file());
}
