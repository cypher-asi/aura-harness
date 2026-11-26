use crate::messages::{summarize_write_input, RedactionMarker};
use serde_json::Value;

#[test]
fn test_summarize_write_file() {
    let input = serde_json::json!({
        "path": "src/main.rs",
        "content": "fn main() { println!(\"hello\"); }"
    });
    let result = summarize_write_input("write_file", &input).unwrap();
    assert_eq!(result["path"], "src/main.rs");
    assert!(result.get("content").is_none());
    assert_eq!(result["_redacted"]["field"], "content");
    assert_eq!(result["_redacted"]["bytes"], 32);
    assert!(RedactionMarker::is_marker(&result));
}

#[test]
fn test_summarize_edit_file() {
    let input = serde_json::json!({
        "path": "src/lib.rs",
        "old_text": "old content here",
        "new_text": "new"
    });
    let result = summarize_write_input("edit_file", &input).unwrap();
    assert_eq!(result["path"], "src/lib.rs");
    assert!(result.get("old_text").is_none());
    assert!(result.get("new_text").is_none());
    assert_eq!(result["_redacted"]["fields"][0]["field"], "old_text");
    assert_eq!(result["_redacted"]["fields"][0]["bytes"], 16);
    assert_eq!(result["_redacted"]["fields"][1]["field"], "new_text");
    assert_eq!(result["_redacted"]["fields"][1]["bytes"], 3);
    assert!(RedactionMarker::is_marker(&result));

    let input_alt = serde_json::json!({
        "path": "src/lib.rs",
        "old_string": "abc",
        "new_string": "defgh"
    });
    let result2 = summarize_write_input("edit_file", &input_alt).unwrap();
    assert!(result2.get("old_string").is_none());
    assert!(result2.get("new_string").is_none());
    assert_eq!(result2["_redacted"]["fields"][0]["field"], "old_string");
    assert_eq!(result2["_redacted"]["fields"][1]["field"], "new_string");
    assert!(result2.get("old_text").is_none());
    assert!(result2.get("new_text").is_none());
}

#[test]
fn test_summarize_unknown_tool() {
    let input = serde_json::json!({"query": "some search"});
    assert!(summarize_write_input("search_code", &input).is_none());
    assert!(summarize_write_input("run_command", &input).is_none());
    assert!(summarize_write_input("totally_unknown", &input).is_none());
}

#[test]
fn marker_is_not_a_string_field() {
    let input = serde_json::json!({
        "path": "src/main.rs",
        "content": "fn main() {}",
    });
    let marked = RedactionMarker::mark(&input, "content", 12);

    assert!(marked.get("content").is_none());
    assert!(marked.get("_redacted").is_some_and(Value::is_object));
    assert!(RedactionMarker::is_marker(&marked));
}
