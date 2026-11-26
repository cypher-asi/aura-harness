//! A step's event read into its row, fields and sections.

use std::collections::HashMap;

use aura_agent_trace::run_log::{LogEvent, LogToolCall};
use aura_core_types::trace::{FileTouch, HarnessAction, TokenUsage};
use aura_ui::events::{LogField, LogStatus};
use serde_json::Value;

use super::super::compare::view::{headline, money, tokens};
use super::step::{section, Parts, Row};
use super::{count, field, pretty, summary};

/// `event`'s row, its fields and sections added to `parts`. Each reader
/// below takes one kind of event and passes on the others.
pub(super) fn event_row(
    parts: &mut Parts<'_>,
    calls: &HashMap<&str, &LogToolCall>,
    event: &LogEvent,
) -> Row {
    prompt_event(parts, event)
        .or_else(|| model_event(parts, event))
        .or_else(|| tool_event(parts, calls, event))
        .or_else(|| decision_event(parts, event))
        .or_else(|| end_event(parts, event))
        // Every kind of event has a reader above.
        .unwrap_or_else(|| Row::new("", String::new(), String::new(), LogStatus::Ok))
}

fn prompt_event(parts: &mut Parts<'_>, event: &LogEvent) -> Option<Row> {
    let LogEvent::UserPrompt { text } = event else {
        return None;
    };
    parts.sections.push(section("Prompt", text, false));
    Some(Row::new(
        "prompt",
        headline(text, 160),
        String::new(),
        LogStatus::Ok,
    ))
}

fn model_event(parts: &mut Parts<'_>, event: &LogEvent) -> Option<Row> {
    let LogEvent::ModelCall {
        model,
        purpose,
        thinking,
        thinking_chars,
        thinking_omitted,
        text,
        tool_calls,
        stop_reason,
        error,
        usage,
        cost_usd,
    } = event
    else {
        return None;
    };
    let (purpose, error) = (purpose.as_deref(), error.as_deref());
    model_details(parts, model, purpose, stop_reason, error, usage, *cost_usd);
    let thinking = (thinking.as_deref(), *thinking_chars, *thinking_omitted);
    model_output(parts, thinking, text, tool_calls);
    parts.tokens = format!(
        "{} tok",
        tokens(usage.input + usage.output + usage.cache_read)
    );
    parts.cost = cost_usd.map(money).unwrap_or_default();
    let label = model_label(purpose, tool_calls, text);
    Some(model_row(label, error, stop_reason))
}

fn tool_event(
    parts: &mut Parts<'_>,
    calls: &HashMap<&str, &LogToolCall>,
    event: &LogEvent,
) -> Option<Row> {
    let LogEvent::ToolResult {
        tool_use_id,
        tool,
        is_error,
        source,
        content_chars,
        output,
        output_complete,
        files,
    } = event
    else {
        return None;
    };
    let output = summary::Output {
        tool,
        text: output,
        complete: *output_complete,
        chars: *content_chars,
        is_error: *is_error,
        source,
        files,
    };
    Some(tool_result(parts, calls, tool_use_id, &output))
}

fn decision_event(parts: &mut Parts<'_>, event: &LogEvent) -> Option<Row> {
    let LogEvent::Decision {
        source,
        name,
        action,
        alternatives,
        trigger,
        features,
    } = event
    else {
        return None;
    };
    Some(decision(
        parts,
        source,
        name,
        action,
        alternatives,
        trigger,
        features,
    ))
}

fn end_event(parts: &mut Parts<'_>, event: &LogEvent) -> Option<Row> {
    let LogEvent::TurnEnd {
        stop_reason,
        iterations,
        input_tokens,
        output_tokens,
        files_changed,
        error,
    } = event
    else {
        return None;
    };
    Some(turn_end(
        parts,
        stop_reason,
        *iterations,
        (*input_tokens, *output_tokens),
        *files_changed,
        error.as_deref(),
    ))
}

/// A model call's model, purpose, stop reason, error, usage and cost.
fn model_details(
    parts: &mut Parts<'_>,
    model: &str,
    purpose: Option<&str>,
    stop_reason: &str,
    error: Option<&str>,
    usage: &TokenUsage,
    cost_usd: Option<f64>,
) {
    parts.fields.push(field("model", model));
    if let Some(purpose) = purpose {
        parts.fields.push(field("for", purpose));
    }
    parts.fields.push(field("stop reason", stop_reason));
    if let Some(error) = error {
        parts.sections.push(section("Error", error, false));
    }
    parts.fields.push(field(
        "tokens",
        format!(
            "{} in, {} out, {} cache read, {} cache write",
            tokens(usage.input),
            tokens(usage.output),
            tokens(usage.cache_read),
            tokens(usage.cache_write)
        ),
    ));
    if let Some(cost) = cost_usd {
        parts.fields.push(field("cost", money(cost)));
    }
}

/// A model call's thinking, reply and tool calls, noting the runs the calls
/// spawned.
fn model_output(
    parts: &mut Parts<'_>,
    (thinking, thinking_chars, thinking_omitted): (Option<&str>, usize, bool),
    text: &str,
    tool_calls: &[LogToolCall],
) {
    match thinking {
        Some(thinking) => parts.sections.push(section("Thinking", thinking, false)),
        None if thinking_omitted => parts.sections.push(section(
            "Thinking",
            format!("{thinking_chars} characters, not kept by this run"),
            false,
        )),
        None => {}
    }
    if !text.is_empty() {
        parts.sections.push(section("Reply", text, false));
    }
    for call in tool_calls {
        let input = serde_json::to_string_pretty(&call.input).unwrap_or_default();
        let mut title = format!("Call {} ({})", call.name, call.tool_use_id);
        if let Some(child) = &call.child_run {
            title.push_str(&format!(" → run {child}"));
            parts
                .spawned
                .extend(parts.below.get(child.as_str()).copied());
        }
        parts.sections.push(section(&title, input, true));
    }
}

/// What a model call's row is labelled with.
fn model_label(purpose: Option<&str>, tool_calls: &[LogToolCall], text: &str) -> String {
    if let Some(purpose) = purpose {
        format!("gate {purpose}")
    } else if !tool_calls.is_empty() {
        let calls: Vec<String> = tool_calls
            .iter()
            .map(|call| summary::call_label(&call.name, &call.input))
            .collect();
        calls.join(" · ")
    } else if text.trim().is_empty() {
        "no reply".to_string()
    } else {
        headline(text, 160)
    }
}

fn model_row(label: String, error: Option<&str>, stop_reason: &str) -> Row {
    if let Some(error) = error {
        Row::new("model", label, headline(error, 160), LogStatus::Error)
    } else if stop_reason == "max_tokens" {
        Row::new(
            "model",
            label,
            "hit its output cap".to_string(),
            LogStatus::Warn,
        )
    } else {
        Row::new("model", label, String::new(), LogStatus::Ok)
    }
}

/// A tool result, labelled by the call in `calls` it answers.
fn tool_result(
    parts: &mut Parts<'_>,
    calls: &HashMap<&str, &LogToolCall>,
    tool_use_id: &str,
    output: &summary::Output<'_>,
) -> Row {
    let fields = &mut parts.fields;
    fields.push(field("tool", output.tool));
    fields.push(field("call", tool_use_id));
    fields.push(field("from", output.source));
    fields.push(field("output", format!("{} characters", output.chars)));
    for file in output.files {
        fields.push(file_field(file));
    }
    let title = if output.complete {
        "Output"
    } else {
        "Output (start; AURA_TRACE=full keeps it whole)"
    };
    parts.sections.push(section(title, output.text, true));
    let label = calls.get(tool_use_id).map_or_else(
        || output.tool.to_string(),
        |call| summary::call_label(&call.name, &call.input),
    );
    let said = summary::result_summary(output);
    let status = if output.is_error {
        LogStatus::Error
    } else if output.source == "blocked" {
        LogStatus::Warn
    } else {
        LogStatus::Ok
    };
    Row::new("tool", label, said, status)
}

fn file_field(file: &FileTouch) -> LogField {
    field(
        "file",
        format!(
            "{} {} (+{} -{})",
            file.kind, file.path, file.added, file.removed
        ),
    )
}

/// A harness decision: what it chose, from what, on what.
fn decision(
    parts: &mut Parts<'_>,
    source: &str,
    name: &str,
    action: &HarnessAction,
    alternatives: &[String],
    trigger: &Value,
    features: &Value,
) -> Row {
    parts.fields.push(field("source", source));
    parts.fields.push(field("decision", name));
    parts.fields.push(field("action", action.label()));
    if !alternatives.is_empty() {
        parts
            .fields
            .push(field("alternatives", alternatives.join(", ")));
    }
    parts.sections.push(section(
        "Action",
        serde_json::to_string_pretty(action).unwrap_or_default(),
        true,
    ));
    if !trigger.is_null() {
        parts
            .sections
            .push(section("Trigger", pretty(trigger), true));
    }
    if !features.is_null() {
        parts
            .sections
            .push(section("Features", pretty(features), true));
    }
    let status = if matches!(
        action,
        HarnessAction::Block { .. } | HarnessAction::Stop { .. } | HarnessAction::Retry { .. }
    ) {
        LogStatus::Warn
    } else {
        LogStatus::Ok
    };
    Row::new(
        "decision",
        format!("{source}/{name} {}", action.label()),
        String::new(),
        status,
    )
}

/// The end of a turn: why it stopped, what it cost and changed.
fn turn_end(
    parts: &mut Parts<'_>,
    stop_reason: &str,
    iterations: usize,
    (input_tokens, output_tokens): (u64, u64),
    files_changed: usize,
    error: Option<&str>,
) -> Row {
    parts.fields.push(field("stop reason", stop_reason));
    parts
        .fields
        .push(field("model calls", iterations.to_string()));
    parts.fields.push(field(
        "tokens",
        format!("{} in, {} out", tokens(input_tokens), tokens(output_tokens)),
    ));
    parts
        .fields
        .push(field("files changed", files_changed.to_string()));
    if let Some(error) = error {
        parts.sections.push(section("Error", error, true));
    }
    let label = format!("turn ended: {stop_reason}");
    match error {
        Some(error) => Row::new(
            "end",
            label,
            format!("error: {}", headline(error, 72)),
            LogStatus::Error,
        ),
        None if files_changed > 0 => Row::new(
            "end",
            label,
            count(files_changed, "file") + " changed",
            LogStatus::Ok,
        ),
        None => Row::new("end", label, String::new(), LogStatus::Ok),
    }
}
