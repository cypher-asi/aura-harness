use super::*;

/// Leaked-tool-markup recovery: when the model writes a tool call as
/// assistant *text* (so `text_sanitize` scrubs it and no tool runs),
/// the turn must re-prompt once instead of ending after a single
/// message on unfinished work. Here the first response leaks
/// `<function_calls>` markup with `EndTurn`; the loop injects the
/// corrective nudge and re-samples, and the second (clean) response
/// terminates the turn — two iterations total.
#[tokio::test]
async fn leaked_tool_markup_triggers_one_retry() {
    let provider = MockProvider::new()
        .with_response(MockResponse::text(
            "I'll read it.\n<function_calls>\n<invoke name=\"read_file\">\n\
             <parameter name=\"path\">a.txt</parameter>\n</invoke>\n</function_calls>",
        ))
        .with_response(MockResponse::text("done"));

    let executor = SuccessExecutor;
    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("read a.txt")];
    let tools = vec![read_file_tool()];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(
        result.iterations, 2,
        "leaked tool markup must trigger exactly one recovery re-sample"
    );

    let has_nudge = result.messages.iter().any(|msg| {
        msg.content.iter().any(|block| {
            matches!(block, ContentBlock::Text { text } if text.contains("native tool-calling"))
        })
    });
    assert!(
        has_nudge,
        "the corrective leaked-tool-markup steer must be appended to history"
    );
}

/// A clean text-only `EndTurn` (no scrubbed markup) must NOT trigger
/// the recovery re-sample — the turn ends after one iteration.
#[tokio::test]
async fn clean_end_turn_does_not_retry() {
    let provider =
        MockProvider::new().with_response(MockResponse::text("all done, no tools needed"));

    let executor = SuccessExecutor;
    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("anything")];
    let tools = vec![read_file_tool()];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(
        result.iterations, 1,
        "a clean end_turn must not be re-prompted"
    );
}

/// The recovery is budget-capped at one nudge: if the re-prompted
/// retry *also* leaks tool markup, the turn ends rather than looping
/// forever. Three leaking responses are queued but only two are
/// consumed (sample 0 → nudge → sample 1 leaks again → budget spent).
#[tokio::test]
async fn leaked_tool_markup_retry_is_budget_capped() {
    let leak = |path: &str, tail: &str| {
        MockResponse::text(format!(
            "<function_calls>\n<invoke name=\"read_file\">\n\
             <parameter name=\"path\">{path}</parameter>\n</invoke>\n</function_calls> {tail}"
        ))
    };
    let provider = MockProvider::new()
        .with_response(leak("a.txt", "first"))
        .with_response(leak("b.txt", "second"))
        .with_response(MockResponse::text("third should never be reached"));

    let executor = SuccessExecutor;
    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("read a.txt")];
    let tools = vec![read_file_tool()];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(
        result.iterations, 2,
        "recovery is capped at one nudge; a second consecutive leak ends the turn"
    );
}
