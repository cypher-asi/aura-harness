//! `aura gate eval`, pinned: what it prints for a sample log, as tables
//! and as JSON, with each of its filters, for a data directory with no
//! log yet, and when the log can't be read.

use std::path::Path;
use std::process::{Command, Output};

/// Four attempts at benchmark tasks and what proved each right or wrong,
/// and a line that is not a line of the log, relative to the workspace
/// root.
const LOG: &str = "tests/fixtures/gate_eval.jsonl";

/// Run `aura gate eval` with `args` in the workspace root, with
/// `AURA_DATA_DIR` set to `data_dir`.
fn eval(data_dir: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aura"))
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .env("AURA_DATA_DIR", data_dir)
        .args(["gate", "eval"])
        .args(args)
        .output()
        .expect("run aura gate eval")
}

/// What `aura gate eval` with `args` printed on [`LOG`], once it exited
/// cleanly.
fn printed(args: &[&str]) -> String {
    let output = eval("unused", &[&["--log", LOG], args].concat());
    assert!(
        output.status.success(),
        "aura gate eval failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("aura prints UTF-8")
}

#[test]
fn gate_eval_prints_the_log_its_unreadable_lines_and_the_report() {
    insta::assert_snapshot!("gate_eval", printed(&[]));
}

#[test]
fn gate_eval_json_prints_the_report_alone() {
    insta::assert_snapshot!("gate_eval_json", printed(&["--json"]));
}

#[test]
fn gate_eval_scores_only_the_attempts_each_filter_lets_through() {
    insta::assert_snapshot!("gate_eval_routed", printed(&["--routed"]));
    insta::assert_snapshot!("gate_eval_passed", printed(&["--passed"]));
    insta::assert_snapshot!("gate_eval_task", printed(&["--task", "django"]));
}

#[test]
fn gate_eval_reads_the_data_directorys_log_and_no_log_is_an_empty_one() {
    let output = eval("no-such-data-dir", &[]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).expect("aura prints UTF-8");
    insta::assert_snapshot!("gate_eval_no_log", text);
}

#[test]
fn a_log_that_cannot_be_read_is_named_in_the_error() {
    let output = eval("unused", &["--log", "tests"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("reading tests"), "{stderr}");
}
