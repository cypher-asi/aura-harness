//! What Gate's own check of a change streams.

use super::{Captured, GATE_SONNET};
use crate::event_loop::gate::followup::{Found, Report};
use crate::event_loop::task_check::CheckRun;
use crate::headless::output::Output;
use crate::headless::OutputFormat;
use serde_json::json;

#[tokio::test]
async fn gates_own_check_streams_what_it_found() {
    let captured = Captured::default();
    let output = Output::new(OutputFormat::StreamJson, Box::new(captured.clone()));
    let check = CheckRun {
        command: "cargo test".to_string(),
        passed: Some(false),
        exit_code: Some(101),
        tail: "test parse ... FAILED".to_string(),
    };

    output.report(Found::Checking("cargo test")).await;
    let checked = Found::Checked {
        model: GATE_SONNET,
        command: "cargo test",
        check: &check,
        signal: check.signal(),
    };
    output.report(checked).await;

    assert_eq!(
        captured.json_lines(),
        [json!({
            "type": "checked",
            "model": GATE_SONNET,
            "command": "cargo test",
            "passed": false,
            "summary": check.describe(),
        })],
        "the check is streamed once it has run, not as it starts"
    );
}
