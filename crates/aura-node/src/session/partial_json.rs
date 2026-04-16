//! Best-effort partial JSON extraction for in-flight tool input.
//!
//! While Anthropic streams `input_json_delta` chunks for a tool-use block
//! (with `eager_input_streaming: true`), the accumulated buffer is not yet
//! valid JSON — the object may be unterminated and the string value we care
//! about may still be arriving. We can't call `serde_json::from_str` until
//! the whole tool block closes, but we still want to ship a useful
//! `ToolCallSnapshot` to the client every chunk so the preview card fills
//! in live instead of staying empty until the end of the turn.
//!
//! For specific known top-level string fields (e.g. `markdown_contents`,
//! `content`, `old_text`, `new_text`, `path`, `title`), we just need to scan
//! for `"key":"..."` and collect the characters that have arrived so far,
//! respecting JSON escape rules.

use serde_json::{Map, Value};

/// Build a best-effort `serde_json::Value` from a (possibly incomplete) tool
/// input JSON buffer.
///
/// First tries a strict `serde_json::from_str`; if that succeeds (the buffer
/// is complete or just happened to parse cleanly), returns the parsed value
/// directly.
///
/// Otherwise falls back to a tool-aware extractor that pulls out the
/// well-known top-level string fields the UI's preview cards consume.
/// Returns an empty object if no relevant fields have appeared yet (e.g. the
/// buffer is still on `{"`).
pub(super) fn parse_partial_tool_input(tool_name: &str, buf: &str) -> Value {
    if let Ok(value) = serde_json::from_str::<Value>(buf) {
        return value;
    }

    let mut obj = Map::new();
    for key in fields_for_tool(tool_name) {
        if let Some(v) = extract_partial_string_field(buf, key) {
            obj.insert((*key).to_string(), Value::String(v));
        }
    }
    Value::Object(obj)
}

/// The top-level string fields whose partial values we want to expose to the
/// UI for each known streaming tool. Order doesn't matter; missing fields
/// are simply skipped.
fn fields_for_tool(tool_name: &str) -> &'static [&'static str] {
    match tool_name {
        "create_spec" | "update_spec" => &["title", "markdown_contents"],
        "write_file" => &["path", "content"],
        "edit_file" => &["path", "old_text", "new_text"],
        // For unknown tools, fall back to a conservative set of common
        // string fields so the snapshot at least carries something. The
        // client merges shallowly so unrelated fields are harmless.
        _ => &["path", "title", "content", "markdown_contents"],
    }
}

/// Extract the current best-effort value of a top-level string field from a
/// partial JSON object buffer.
///
/// Returns `None` if the `"key":"` pattern has not yet appeared. Returns the
/// decoded (unescaped) string value built from the characters seen so far;
/// this may be empty if the opening quote has been emitted but no characters
/// have followed yet.
///
/// Handles standard JSON string escapes: `\n`, `\r`, `\t`, `\"`, `\\`, `\/`,
/// `\b`, `\f`, and `\uXXXX` (basic BMP). If a backslash or `\uXXXX` escape
/// is split across the buffer boundary, the partial escape is dropped from
/// the returned value rather than being mis-decoded; the next call with more
/// bytes will pick it up cleanly.
fn extract_partial_string_field(buf: &str, key: &str) -> Option<String> {
    let needle_a = format!("\"{key}\":\"");
    let needle_b = format!("\"{key}\": \"");
    let start = buf
        .find(&needle_a)
        .map(|i| i + needle_a.len())
        .or_else(|| buf.find(&needle_b).map(|i| i + needle_b.len()))?;

    let mut out = String::new();
    let bytes = buf.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            return Some(out);
        }
        if b == b'\\' {
            if i + 1 >= bytes.len() {
                return Some(out);
            }
            let esc = bytes[i + 1];
            match esc {
                b'"' => {
                    out.push('"');
                    i += 2;
                }
                b'\\' => {
                    out.push('\\');
                    i += 2;
                }
                b'/' => {
                    out.push('/');
                    i += 2;
                }
                b'n' => {
                    out.push('\n');
                    i += 2;
                }
                b't' => {
                    out.push('\t');
                    i += 2;
                }
                b'r' => {
                    out.push('\r');
                    i += 2;
                }
                b'b' => {
                    out.push('\u{0008}');
                    i += 2;
                }
                b'f' => {
                    out.push('\u{000C}');
                    i += 2;
                }
                b'u' => {
                    if i + 6 > bytes.len() {
                        return Some(out);
                    }
                    let hex = std::str::from_utf8(&bytes[i + 2..i + 6]).unwrap_or("");
                    if let Ok(code) = u32::from_str_radix(hex, 16) {
                        if let Some(ch) = char::from_u32(code) {
                            out.push(ch);
                            i += 6;
                            continue;
                        }
                    }
                    out.push('\u{FFFD}');
                    i += 6;
                }
                _ => {
                    out.push(esc as char);
                    i += 2;
                }
            }
            continue;
        }
        match std::str::from_utf8(&bytes[i..]) {
            Ok(_) => {
                let ch = buf[i..].chars().next().expect("non-empty suffix");
                out.push(ch);
                i += ch.len_utf8();
            }
            Err(e) => {
                let valid_up_to = e.valid_up_to();
                if valid_up_to == 0 {
                    return Some(out);
                }
                let chunk = &buf[i..i + valid_up_to];
                for ch in chunk.chars() {
                    if ch == '"' {
                        return Some(out);
                    }
                    out.push(ch);
                }
                return Some(out);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_complete_json_when_buffer_already_parses() {
        let v = parse_partial_tool_input("create_spec", r#"{"title":"Done"}"#);
        assert_eq!(v["title"], "Done");
    }

    #[test]
    fn extracts_partial_markdown_contents_for_create_spec() {
        let v = parse_partial_tool_input(
            "create_spec",
            "{\"title\":\"My Spec\",\"markdown_contents\":\"# Hello\\n\\nworld",
        );
        assert_eq!(v["title"], "My Spec");
        assert_eq!(v["markdown_contents"], "# Hello\n\nworld");
    }

    #[test]
    fn extracts_partial_content_for_write_file() {
        let v = parse_partial_tool_input(
            "write_file",
            "{\"path\":\"src/a.ts\",\"content\":\"export const x = 1;\\nexport const y",
        );
        assert_eq!(v["path"], "src/a.ts");
        assert_eq!(v["content"], "export const x = 1;\nexport const y");
    }

    #[test]
    fn extracts_partial_diff_fields_for_edit_file() {
        let v = parse_partial_tool_input(
            "edit_file",
            "{\"path\":\"a.ts\",\"old_text\":\"foo\",\"new_text\":\"ba",
        );
        assert_eq!(v["path"], "a.ts");
        assert_eq!(v["old_text"], "foo");
        assert_eq!(v["new_text"], "ba");
    }

    #[test]
    fn returns_empty_object_when_no_fields_present_yet() {
        let v = parse_partial_tool_input("create_spec", "{\"unrelated\":\"x");
        assert_eq!(v, serde_json::json!({}));
    }

    #[test]
    fn unknown_tool_falls_back_to_common_fields() {
        let v = parse_partial_tool_input("custom_tool", "{\"title\":\"hi");
        assert_eq!(v["title"], "hi");
    }

    #[test]
    fn drops_trailing_lone_backslash_at_boundary() {
        let v = parse_partial_tool_input(
            "create_spec",
            "{\"markdown_contents\":\"ab\\",
        );
        assert_eq!(v["markdown_contents"], "ab");
    }

    #[test]
    fn drops_partial_unicode_escape_at_boundary() {
        let v = parse_partial_tool_input(
            "create_spec",
            "{\"markdown_contents\":\"ab\\u00",
        );
        assert_eq!(v["markdown_contents"], "ab");
    }

    #[test]
    fn handles_grow_across_calls() {
        let mut buf = String::from("{\"markdown_contents\":\"");
        let v = parse_partial_tool_input("create_spec", &buf);
        assert_eq!(v["markdown_contents"], "");

        buf.push_str("# H");
        let v = parse_partial_tool_input("create_spec", &buf);
        assert_eq!(v["markdown_contents"], "# H");

        buf.push_str("ello\\n");
        let v = parse_partial_tool_input("create_spec", &buf);
        assert_eq!(v["markdown_contents"], "# Hello\n");

        buf.push_str("world\"}");
        let v = parse_partial_tool_input("create_spec", &buf);
        assert_eq!(v["markdown_contents"], "# Hello\nworld");
    }
}
