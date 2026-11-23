//! What a run prints and streams, in each output format.

use super::super::SessionChoice;
use super::{fix_the_parser, todo_call};
use crate::headless::output::ExitReason;
use crate::headless::tests::{dirs, execute, failing_check_provider, headless, request};
use crate::headless::tools::ToolFilter;
use crate::headless::OutputFormat;
use aura_model::{MockProvider, MockResponse};

#[tokio::test]
async fn text_prints_only_the_final_reply() {
    let dirs = dirs();
    let (mut h, _) = headless(
        &dirs,
        MockProvider::simple_response("All done."),
        ToolFilter::default(),
    );

    let (report, out) = execute(
        &mut h,
        request("hi", SessionChoice::New),
        OutputFormat::Text,
    )
    .await;

    assert_eq!(report.exit_reason, ExitReason::Success);
    assert_eq!(out.text(), "All done.\n");
}

#[tokio::test]
async fn json_reports_usage_todos_and_session() {
    let dirs = dirs();
    let provider = MockProvider::new()
        .with_response(todo_call())
        .with_response(MockResponse::text("Finished."));
    let (mut h, _) = headless(&dirs, provider, ToolFilter::default());

    let (report, out) = execute(
        &mut h,
        request("build it", SessionChoice::New),
        OutputFormat::Json,
    )
    .await;

    let lines = out.json_lines();
    assert_eq!(lines.len(), 1, "{}", out.text());
    let json = &lines[0];
    assert_eq!(json["type"], "result");
    assert_eq!(json["exit_reason"], "success");
    assert_eq!(json["is_error"], false);
    assert_eq!(json["result"], "Finished.");
    assert_eq!(json["session_id"], report.session_id.as_str());
    assert!(report.session_id.contains(':'));
    assert_eq!(json["num_turns"], 2);
    assert_eq!(json["agent_turns"], 1);
    assert_eq!(json["usage"]["calls"], 2);
    assert!(json["usage"]["output_tokens"].as_u64().unwrap() > 0);
    assert!(json["usage"].get("cache_read_tokens").is_some());
    assert_eq!(json["compactions"], 0);
    assert_eq!(json["todos"][0]["content"], "Write the parser");
    assert_eq!(json["todos"][0]["status"], "completed");
}

#[tokio::test]
async fn stream_json_streams_events_and_ends_with_the_result() {
    let dirs = dirs();
    let provider = MockProvider::new()
        .with_response(todo_call())
        .with_response(MockResponse::text("Finished."));
    let (mut h, _) = headless(&dirs, provider, ToolFilter::default());

    let (_, out) = execute(
        &mut h,
        request("build it", SessionChoice::New),
        OutputFormat::StreamJson,
    )
    .await;

    let lines = out.json_lines();
    let types: Vec<&str> = lines.iter().map(|l| l["type"].as_str().unwrap()).collect();
    assert_eq!(types.first(), Some(&"init"), "{types:?}");
    assert_eq!(types.last(), Some(&"result"), "{types:?}");
    assert!(lines[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t == "todo_write"));
    let result = lines
        .iter()
        .find(|l| l["type"] == "tool_result")
        .expect("a tool_result line");
    assert_eq!(result["name"], "todo_write");
    assert_eq!(result["input"]["todos"][0]["id"], "1");
    assert_eq!(result["is_error"], false);
    assert!(types.contains(&"todos"), "{types:?}");
    assert_eq!(
        types.iter().filter(|t| **t == "usage").count(),
        2,
        "{}",
        out.text()
    );
    let streamed: String = lines
        .iter()
        .filter(|l| l["type"] == "text_delta")
        .filter_map(|l| l["text"].as_str())
        .collect();
    assert_eq!(streamed, "Finished.");
}

#[tokio::test]
async fn a_move_up_prints_its_report_byte_for_byte() {
    let then = [
        "The parser still drops the last token.\nVERDICT: MODEL",
        "Fixed it.",
    ];
    let run = fix_the_parser(failing_check_provider(&then)).await;

    let report = &run.report;
    let printed = format!(
        "{{\"type\":\"result\",\"exit_reason\":\"success\",\"is_error\":false,\"result\":\
         \"Fixed it.\",\"session_id\":\"{}\",\"model\":\"aura-claude-opus-4-7\",\
         \"num_turns\":4,\"agent_turns\":2,\"duration_ms\":{},\"usage\":{{\"calls\":4,\
         \"input_tokens\":400,\"cache_read_tokens\":0,\"cache_write_tokens\":0,\
         \"output_tokens\":200,\"cache_hit_rate\":0.0,\"cost_usd\":0.0049}},\
         \"total_cost_usd\":0.0098,\"gate_cost_usd\":0.0049,\"compactions\":0,\"todos\":[],\
         \"files_changed\":[\"parser.txt\"],\"error\":null}}",
        report.session_id, report.duration_ms,
    );
    assert_eq!(run.printed.last(), Some(&printed));
}
