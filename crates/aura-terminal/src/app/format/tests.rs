use super::*;
use serde_json::json;

fn envelope(ok: bool, exit_code: i32, stdout: &str, stderr: &str) -> String {
    json!({ "ok": ok, "exit_code": exit_code, "stdout": stdout, "stderr": stderr }).to_string()
}

#[test]
fn run_command_shows_stdout() {
    let result = envelope(true, 0, "Cargo.toml\nsrc\n", "");
    assert_eq!(format_tool_result("run_command", &result), "Cargo.toml\nsrc");
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
fn other_tools_are_unchanged() {
    let result = envelope(true, 0, "x", "");
    assert_eq!(format_tool_result("read_file", &result), result);
}
