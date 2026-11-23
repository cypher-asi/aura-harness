//! The limits that stop a run that keeps going: its model calls and its
//! time.

use super::super::SessionChoice;
use super::todo_call;
use crate::headless::output::ExitReason;
use crate::headless::tests::{dirs, execute, headless, request};
use crate::headless::tools::ToolFilter;
use crate::headless::OutputFormat;
use aura_model::MockProvider;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_turns_stops_a_run_that_keeps_calling_tools() {
    let dirs = dirs();
    // Latency lets the limit land before the next call returns.
    let provider = MockProvider::new()
        .with_default_response(todo_call())
        .with_latency(50);
    let (mut h, _) = headless(&dirs, provider, ToolFilter::default());
    let mut req = request("loop", SessionChoice::New);
    req.limits.max_turns = Some(2);

    let (report, _) = execute(&mut h, req, OutputFormat::Json).await;

    assert_eq!(report.exit_reason, ExitReason::MaxTurns);
    assert_eq!(report.exit_reason.exit_code(), 3);
    assert!(report.is_error);
    assert_eq!(report.num_turns, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_time_stops_a_long_run_and_keeps_its_session() {
    let dirs = dirs();
    let provider = MockProvider::new()
        .with_default_response(todo_call())
        .with_latency(100);
    let (mut h, _) = headless(&dirs, provider, ToolFilter::default());
    let mut req = request("loop", SessionChoice::New);
    req.limits.max_time = Some(Duration::from_millis(400));

    let (report, _) = execute(&mut h, req, OutputFormat::Json).await;

    assert_eq!(report.exit_reason, ExitReason::MaxTime);
    assert!(report.num_turns >= 1);
    assert_eq!(report.todos.len(), 1, "the stopped turn's work was kept");
}
