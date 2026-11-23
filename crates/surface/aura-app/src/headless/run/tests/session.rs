//! Session ids, and the sessions `--resume` and `--continue` pick up.

use super::super::{parse_session_id, session_id, SessionChoice};
use super::{last_prompts, todo_call};
use crate::headless::output::{ExitReason, RunReport};
use crate::headless::tests::{dirs, execute, headless, request, Dirs, Requests};
use crate::headless::tools::ToolFilter;
use crate::headless::OutputFormat;
use aura_core_types::AgentId;
use aura_model::{Message, MockProvider, MockResponse};

#[test]
fn session_ids_round_trip() {
    let agent = AgentId::generate();
    let id = session_id(agent, 1_700_000_000_000);
    assert_eq!(
        parse_session_id(&id, AgentId::generate()),
        Ok((agent, 1_700_000_000_000))
    );
    let other = AgentId::generate();
    assert_eq!(parse_session_id(" 12 ", other), Ok((other, 12)));
    assert!(parse_session_id("nope", other).is_err());
    assert!(parse_session_id("zz:12", other).is_err());
}

/// Run `prompt` on `dirs` in the session `session` names, the model
/// answering with `provider`.
async fn run_in(
    dirs: &Dirs,
    provider: MockProvider,
    prompt: &str,
    session: SessionChoice,
) -> (RunReport, Requests) {
    let (mut h, requests) = headless(dirs, provider, ToolFilter::default());
    let (report, _) = execute(&mut h, request(prompt, session), OutputFormat::Json).await;
    (report, requests)
}

#[tokio::test]
async fn resume_and_continue_pick_up_a_saved_session() {
    let dirs = dirs();
    let provider = MockProvider::new()
        .with_response(todo_call())
        .with_response(MockResponse::text("Noted."));
    let (first, _) = run_in(&dirs, provider, "remember 42", SessionChoice::New).await;
    let first = first.session_id;

    // A new run starts over…
    let fresh = MockProvider::simple_response("Fresh.");
    let (report, requests) = run_in(&dirs, fresh, "unrelated", SessionChoice::New).await;
    assert_ne!(report.session_id, first);
    assert_eq!(last_prompts(&requests), ["unrelated"]);
    assert!(report.todos.is_empty());

    // …and `--resume` brings the first one back, tool calls and todos
    // included.
    let recalled = MockProvider::simple_response("It was 42.");
    let resume = SessionChoice::Resume(first.clone());
    let (report, requests) = run_in(&dirs, recalled, "what was it?", resume).await;
    assert_eq!(report.exit_reason, ExitReason::Success);
    assert_eq!(report.session_id, first);
    assert_eq!(last_prompts(&requests), ["remember 42", "what was it?"]);
    assert_eq!(report.todos.len(), 1);
    let sent = requests.lock().unwrap()[0].clone();
    assert!(sent.messages.iter().any(Message::has_tool_use));

    // `--continue` keeps going in the session the last run used.
    let still = MockProvider::simple_response("Still 42.");
    let (report, requests) = run_in(&dirs, still, "and now?", SessionChoice::Continue).await;
    assert_eq!(report.session_id, first);
    assert_eq!(
        last_prompts(&requests),
        ["remember 42", "what was it?", "and now?"]
    );
}

#[tokio::test]
async fn resuming_an_unknown_session_fails() {
    let dirs = dirs();
    let (mut h, _) = headless(
        &dirs,
        MockProvider::simple_response("x"),
        ToolFilter::default(),
    );
    let missing = session_id(AgentId::generate(), 1);

    let (report, out) = execute(
        &mut h,
        request("hi", SessionChoice::Resume(missing)),
        OutputFormat::Json,
    )
    .await;

    assert_eq!(report.exit_reason, ExitReason::Error);
    assert_eq!(report.exit_reason.exit_code(), 1);
    assert_eq!(out.json_lines()[0]["is_error"], true);
}
