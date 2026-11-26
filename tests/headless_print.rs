//! `aura -p` end to end: the real binary with the mock provider, run in a
//! scratch project with its own data directory and `AURA_HOME`.

// Integration-test crate: test code may unwrap/expect.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

struct Project {
    workspace: TempDir,
    data: TempDir,
    home: TempDir,
}

impl Project {
    fn new() -> Self {
        Self {
            workspace: TempDir::new().expect("workspace dir"),
            data: TempDir::new().expect("data dir"),
            home: TempDir::new().expect("aura home"),
        }
    }

    fn aura(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_aura"))
            .args(["--provider", "mock"])
            .args(args)
            .current_dir(self.workspace.path())
            .env("AURA_DATA_DIR", self.data.path())
            .env("AURA_HOME", self.home.path())
            .env_remove("RUST_LOG")
            .output()
            .expect("run aura")
    }
}

fn json(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout is not one JSON object ({e}): {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn print_text_writes_only_the_reply() {
    let project = Project::new();
    let output = project.aura(&["-p", "hello"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Mock provider (tests only)\n"
    );
}

#[test]
fn print_json_then_continue_the_same_session() {
    let project = Project::new();
    let first = project.aura(&["-p", "remember this", "--output-format", "json"]);
    assert!(first.status.success(), "{first:?}");
    let first = json(&first);
    assert_eq!(first["type"], "result");
    assert_eq!(first["exit_reason"], "success");
    assert_eq!(first["num_turns"], 1);
    let session = first["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let again = project.aura(&["-p", "and now?", "--continue", "--output-format", "json"]);
    assert!(again.status.success(), "{again:?}");
    assert_eq!(json(&again)["session_id"], session.as_str());

    let resumed = project.aura(&[
        "-p",
        "once more",
        "--resume",
        &session,
        "--output-format",
        "json",
    ]);
    assert!(resumed.status.success(), "{resumed:?}");
    assert_eq!(json(&resumed)["session_id"], session.as_str());
}

#[test]
fn print_stream_json_starts_with_init_and_ends_with_result() {
    let project = Project::new();
    let output = project.aura(&["-p", "hi", "--output-format", "stream-json"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is JSON"))
        .collect();
    assert_eq!(
        lines.first().map(|l| l["type"].clone()),
        Some("init".into())
    );
    assert_eq!(
        lines.last().map(|l| l["type"].clone()),
        Some("result".into())
    );
}

#[test]
fn print_reads_the_prompt_from_stdin_and_honours_cwd() {
    let project = Project::new();
    let elsewhere = TempDir::new().expect("other dir");
    let mut child = Command::new(env!("CARGO_BIN_EXE_aura"))
        .args([
            "--provider",
            "mock",
            "-p",
            "--output-format",
            "json",
            "--cwd",
        ])
        .arg(project.workspace.path())
        .current_dir(elsewhere.path())
        .env("AURA_DATA_DIR", project.data.path())
        .env("AURA_HOME", project.home.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn aura");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(b"from stdin").expect("write prompt");
    }
    let output = child.wait_with_output().expect("wait for aura");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(json(&output)["exit_reason"], "success");
    assert!(!Path::new(&elsewhere.path().join("aura_data")).exists());
}

#[test]
fn print_failures_exit_non_zero_with_a_json_error() {
    let project = Project::new();
    let output = project.aura(&["-p", "hi", "--resume", "123", "--output-format", "json"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = json(&output);
    assert_eq!(report["is_error"], true);
    assert!(report["error"].as_str().is_some_and(|e| e.contains("123")));

    let usage = project.aura(&["--max-turns", "2"]);
    assert_eq!(
        usage.status.code(),
        Some(2),
        "flags without -p are a usage error"
    );
}
