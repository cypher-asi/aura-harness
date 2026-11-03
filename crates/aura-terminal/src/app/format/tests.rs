use super::*;
use serde_json::json;

fn envelope(ok: bool, exit_code: i32, stdout: &str, stderr: &str) -> String {
    json!({ "ok": ok, "exit_code": exit_code, "stdout": stdout, "stderr": stderr }).to_string()
}

#[test]
fn run_command_shows_stdout() {
    let result = envelope(true, 0, "Cargo.toml\nsrc\n", "");
    assert_eq!(
        format_tool_result("run_command", &result),
        "Cargo.toml\nsrc"
    );
}

#[test]
fn run_command_without_output_says_so() {
    let result = envelope(true, 0, "", "");
    assert_eq!(format_tool_result("run_command", &result), "(no output)");
}

#[test]
fn failed_run_command_shows_exit_code_and_stderr() {
    let result = envelope(false, 2, "", "ls: cannot access 'x'\n");
    assert_eq!(
        format_tool_result("run_command", &result),
        "exit 2\nstderr:\nls: cannot access 'x'"
    );
}

#[test]
fn non_envelope_result_is_unchanged() {
    assert_eq!(
        format_tool_result("run_command", "Kernel error"),
        "Kernel error"
    );
}

#[test]
fn tool_labels_say_what_the_call_did() {
    let label = |name: &str, args: serde_json::Value| format_tool_label(name, &args.to_string());
    assert_eq!(
        label(
            "read_file",
            json!({ "path": "crates/a/src/provider.rs", "start_line": 1, "end_line": 383 })
        ),
        "Read provider.rs L1-383"
    );
    assert_eq!(
        label("read_file", json!({ "path": "main.rs" })),
        "Read main.rs"
    );
    assert_eq!(
        label(
            "search_code",
            json!({ "pattern": "LinkedAccount", "path": "crates", "include": "*.rs" })
        ),
        "Grepped LinkedAccount in crates (*.rs)"
    );
    assert_eq!(
        label("find_files", json!({ "pattern": "*.toml" })),
        "Found *.toml"
    );
    assert_eq!(label("list_files", json!({})), "Listed .");
    assert_eq!(
        label(
            "edit_file",
            json!({ "path": "src/lib.rs", "old_text": "a" })
        ),
        "Edited lib.rs"
    );
    assert_eq!(
        label("run_command", json!({ "command": "cargo test" })),
        "Ran cargo test"
    );
    assert_eq!(label("git_push", json!({})), "git_push");
}

#[test]
fn tool_label_waits_for_complete_args() {
    assert_eq!(format_tool_label("read_file", r#"{"path":"ma"#), "");
}

#[test]
fn tool_label_uses_fields_that_finished_streaming() {
    let streaming = r#"{"path":"web/index.html","content":"<html>\n<script>const a = \"x"#;
    assert_eq!(format_tool_label("write_file", streaming), "Wrote index.html");
    assert_eq!(
        format_tool_label("run_command", r#"{"command":"npm i","cwd":"we"#),
        "Ran npm i"
    );
    let nested = r#"{"args":["a","b"],"path":"src","pattern":"T"#;
    assert_eq!(
        parse_tool_args(nested),
        json!({ "path": "src" }),
        "nested values and unfinished strings are left out"
    );
}

#[test]
fn other_tools_are_unchanged() {
    let result = envelope(true, 0, "x", "");
    assert_eq!(format_tool_result("read_file", &result), result);
}
