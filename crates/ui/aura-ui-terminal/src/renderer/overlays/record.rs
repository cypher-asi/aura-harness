//! The lines of a record's details, and its status glyph.

use super::super::markdown::{parse_content_segments, render_markdown, ContentSegment};
use crate::{
    components::{highlight_code_line, CodeBlock, Info, Value},
    Theme,
};
use ratatui::{
    style::{Color, Style},
    text::Line,
};

/// Status glyph and color shown in a record's title.
pub(in crate::renderer) const fn record_status(
    record: &crate::events::RecordSummary,
    theme: &Theme,
) -> (&'static str, Color) {
    use crate::events::RecordStatus;
    match record.status {
        RecordStatus::Ok => ("✓", theme.colors.success),
        RecordStatus::Error => ("✗", theme.colors.error),
        RecordStatus::Pending => ("◌", theme.colors.pending),
        RecordStatus::None => ("·", theme.colors.muted),
    }
}

/// Columns of a label in the RECORD and PROCESS views, whose labels are
/// longer than a panel's.
pub(in crate::renderer) const DETAIL_LABEL_WIDTH: usize = 14;

/// Every line of a record's details, laid out `width` columns wide.
pub(in crate::renderer) fn record_detail_lines(
    record: &crate::events::RecordSummary,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut info = Info::new(width, theme)
        .label_width(DETAIL_LABEL_WIDTH)
        .wrap_values();
    info.section("RECORD", None);
    info.fact("Sequence", &record.seq.to_string(), Value::Plain);
    info.fact("Context hash", &record.full_hash, Value::Quiet);

    info.section("TRANSACTION", None);
    info.fact("Tx id", &record.tx_id, Value::Quiet);
    info.fact("Agent id", &record.agent_id, Value::Quiet);
    let time = format!("{} ({})", record.ts_ms, record.timestamp);
    info.fact("Time", &time, Value::Plain);
    info.fact("Kind", &record.tx_kind, Value::Plain);
    if !record.info.is_empty() {
        info.fact("Info", &record.info, Value::Plain);
    }

    info.section("PROCESSING", None);
    info.fact("Sender", &record.sender, Value::Plain);
    info.fact("Actions", &record.action_count.to_string(), Value::Plain);
    info.fact("Effects", &record.effect_status, Value::Plain);

    if let Some(policy) = &record.policy {
        policy_facts(&mut info, policy);
    }
    if !record.error_details.is_empty() {
        info.section("ERROR", None);
        info.text(&record.error_details, Value::Error);
    }
    info.section("MESSAGE", None);
    let message = message_lines(&record.message, info.indented_width(), theme);
    info.indented(message);
    info.into_lines()
}

/// What the kernel decided about a tool, by which rule, and the checks it
/// went through to get there.
fn policy_facts(info: &mut Info, policy: &crate::events::PolicyDetail) {
    let verdict = match policy.verdict.as_str() {
        "approved" => Value::Plain,
        "denied" => Value::Error,
        _ => Value::Flag,
    };
    info.section("POLICY DECISION", None);
    info.fact("Tool", &policy.tool, Value::Plain);
    info.fact("Verdict", &policy.verdict, verdict);
    info.fact("Decided by", &policy.decided_by, Value::Plain);
    if !policy.reason.is_empty() {
        info.fact("Reason", &policy.reason, Value::Plain);
    }
    if !policy.approval.is_empty() {
        info.fact("Approved by", &policy.approval, Value::Plain);
    }
    info.fact("Mode", &policy.mode, Value::Plain);
    info.fact("Proposal tx", &policy.proposal_tx, Value::Quiet);
    info.fact("Policy hash", &policy.policy_hash, Value::Quiet);
    for (index, check) in policy.trace.iter().enumerate() {
        let label = if index == 0 { "Checks" } else { "" };
        info.fact(label, check, Value::Plain);
    }
}

/// A record's message laid out `width` columns wide: JSON highlighted,
/// anything else as markdown with its code blocks.
fn message_lines(message: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    if message.starts_with(['{', '[']) {
        // Highlight the whole document in order so keys on later lines are
        // recognized as keys (inside their object), not stray strings.
        return match crate::components::syntax::syntax_for_token("json") {
            Some(syntax) => {
                let mut highlighter = crate::components::syntax::LineHighlighter::new(syntax);
                message
                    .lines()
                    .map(|line| Line::from(highlighter.spans(line, theme)))
                    .collect()
            }
            None => message
                .lines()
                .map(|line| Line::from(highlight_code_line("json", line, theme)))
                .collect(),
        };
    }
    let base_style = Style::default().fg(theme.colors.foreground);
    let mut lines = Vec::new();
    for segment in parse_content_segments(message) {
        match segment {
            ContentSegment::Text(text) => {
                lines.extend(render_markdown(&text, width, base_style, theme));
            }
            ContentSegment::CodeBlock { language, code } => {
                lines.extend(CodeBlock::new(language, code).render(theme, width));
            }
        }
    }
    lines
}
