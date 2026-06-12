//! Defensive scrubbing of tool-call markup that a model occasionally
//! emits as assistant *text* instead of as a native `tool_use` block.
//!
//! When a model writes Anthropic tool-call syntax — `<function_calls>` /
//! `<invoke name="...">` / `<parameter name="...">` XML, or the hybrid
//! `[tool_use <name> ... name="...">...</invoke>` shape it sometimes
//! mangles that into — inside a *text* content block, the provider
//! streams it as ordinary text. It is never parsed as a tool call, so it
//! would otherwise be folded verbatim into the conversation history and
//! re-fed to the model on the next iteration (reinforcing the behaviour)
//! and surfaced to the client as raw text.
//!
//! We strip the markup from the assistant message before it enters
//! `state.messages` (see `iteration::accumulate_response`). The client
//! mirror lives in `aura-os` at `interface/src/utils/tool-markers.ts`
//! (display) and `.../persist_task/sanitize.rs` (persistence).

use aura_model_reasoner::{ContentBlock, Message};

/// XML tag names whose `<tag ...>` / `</tag>` forms may leak from the
/// model into assistant text (Anthropic tool-call syntax).
const XML_TOOL_TAGS: [&str; 3] = ["function_calls", "invoke", "parameter"];

/// Return a sanitized clone of `message` with leaked tool-call markup
/// stripped from every `Text` block, plus a `bool` reporting whether any
/// block changed. Non-text blocks (real `tool_use`, `thinking`, …) are
/// preserved verbatim. When nothing changed the caller can keep using
/// the original message; we still return an owned clone for a uniform
/// call shape.
pub(super) fn sanitize_message(message: &Message) -> (Message, bool) {
    let mut changed = false;
    let content = message
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => {
                let (scrubbed, block_changed) = scrub_tool_markup(text);
                changed |= block_changed;
                ContentBlock::Text { text: scrubbed }
            }
            other => other.clone(),
        })
        .collect();
    (Message::new(message.role.clone(), content), changed)
}

/// Strip leaked tool-call markup from a single text fragment.
///
/// Removes Anthropic XML tool-call tags and the hybrid
/// `[tool_use <name> ... name="...">` opener the model sometimes mangles
/// the markup into. Returns the cleaned text and whether anything was
/// removed.
///
/// Conservative by construction: a valid compaction-style marker
/// (`[tool_use <name> input={...}]`) carries `input=` and a closing `]`
/// and never a `name="..."` attribute followed by `>`, so it is left
/// untouched.
pub(super) fn scrub_tool_markup(text: &str) -> (String, bool) {
    if !text.contains('<') && !text.contains("[tool_use") {
        return (text.to_string(), false);
    }

    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if rest.starts_with('<') {
            if let Some(len) = match_xml_tool_tag(rest) {
                changed = true;
                i += len;
                continue;
            }
        } else if rest.starts_with("[tool_use ") {
            if let Some(len) = match_hybrid_tool_use(rest) {
                changed = true;
                i += len;
                continue;
            }
        }
        let ch = rest.chars().next().expect("non-empty rest has a char");
        out.push(ch);
        i += ch.len_utf8();
    }

    (collapse_blank_lines(&out), changed)
}

/// If `rest` opens an XML tool tag (`<tag ...>` or `</tag>` for a known
/// tool tag name), return the byte length of the whole tag through its
/// closing `>`. Returns `None` for any other `<...>` so ordinary prose
/// and code (generics, comparisons) are preserved.
fn match_xml_tool_tag(rest: &str) -> Option<usize> {
    let after_lt = &rest[1..];
    let after_slash = after_lt.strip_prefix('/').unwrap_or(after_lt);
    let name_len = after_slash
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(after_slash.len());
    let name = &after_slash[..name_len];
    if !XML_TOOL_TAGS.contains(&name) {
        return None;
    }
    rest.find('>').map(|gt| gt + 1)
}

/// If `rest` opens a hybrid `[tool_use <name> ... name="...">` marker (an
/// XML `name="..."` attribute plus a `>`, all before any `]` or
/// newline), return the byte length through that `>`. A valid compaction
/// marker has neither `name="` nor a `>` before its `]`, so it returns
/// `None`.
fn match_hybrid_tool_use(rest: &str) -> Option<usize> {
    let bound = rest
        .find(|c: char| c == ']' || c == '\n' || c == '\r')
        .unwrap_or(rest.len());
    let window = &rest[..bound];
    if !window.contains("name=\"") {
        return None;
    }
    window.find('>').map(|gt| gt + 1)
}

/// Collapse runs of 3+ newlines (left behind when a stripped tag occupied
/// its own line) down to a paragraph break.
fn collapse_blank_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut newline_run = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                out.push(ch);
            }
        } else {
            newline_run = 0;
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_model_reasoner::Role;

    #[test]
    fn leaves_plain_text_untouched() {
        let (out, changed) = scrub_tool_markup("compare a < b and note x > y");
        assert_eq!(out, "compare a < b and note x > y");
        assert!(!changed);
    }

    #[test]
    fn strips_well_formed_invoke_block() {
        let input = "I will read it.\n<function_calls>\n<invoke name=\"read_file\">\n<parameter name=\"path\">src/Nav.tsx</parameter>\n</invoke>\n</function_calls>\nDone.";
        let (out, changed) = scrub_tool_markup(input);
        assert!(changed);
        assert!(!out.contains("invoke"));
        assert!(!out.contains("function_calls"));
        assert!(!out.contains("parameter"));
        assert!(out.contains("I will read it."));
        assert!(out.contains("Done."));
    }

    #[test]
    fn strips_reported_hybrid_marker() {
        let input = "I'll inspect the nav. [tool_use read_file name=\"Nav.tsx\"> </invoke>";
        let (out, changed) = scrub_tool_markup(input);
        assert!(changed);
        assert!(!out.contains("tool_use"));
        assert!(!out.contains("invoke"));
        assert!(out.contains("I'll inspect the nav."));
    }

    #[test]
    fn preserves_valid_compaction_marker() {
        let input = "[tool_use read_file input={\"path\":\"src/db.rs\"}]";
        let (out, changed) = scrub_tool_markup(input);
        assert_eq!(out, input);
        assert!(!changed);
    }

    #[test]
    fn preserves_compaction_marker_with_name_key_in_json() {
        let input = "[tool_use list_tasks input={\"name\":\"x\"}]";
        let (out, changed) = scrub_tool_markup(input);
        assert_eq!(out, input);
        assert!(!changed);
    }

    #[test]
    fn sanitize_message_only_touches_text_blocks() {
        let message = Message::new(
            Role::Assistant,
            vec![
                ContentBlock::Text {
                    text: "look <invoke name=\"read_file\"><parameter name=\"path\">a</parameter></invoke>"
                        .to_string(),
                },
                ContentBlock::ToolUse {
                    id: "toolu_1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({ "path": "a" }),
                },
            ],
        );

        let (sanitized, changed) = sanitize_message(&message);
        assert!(changed);
        match &sanitized.content[0] {
            ContentBlock::Text { text } => {
                assert!(!text.contains("invoke"));
                assert!(text.contains("look"));
            }
            other => panic!("expected text block, got {other:?}"),
        }
        // The real tool_use block is preserved verbatim.
        match &sanitized.content[1] {
            ContentBlock::ToolUse { id, name, .. } => {
                assert_eq!(id, "toolu_1");
                assert_eq!(name, "read_file");
            }
            other => panic!("expected tool_use block, got {other:?}"),
        }
    }
}
