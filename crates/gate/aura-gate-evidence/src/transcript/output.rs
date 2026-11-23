//! What a command printed, read once from its result.

use aura_model::ToolResultContent;
use serde_json::{Map, Value};

/// Field (in a JSON result) or line prefix (in a text one) with the exit
/// code of a failed `run_command` call that ran to its end.
const EXIT_CODE: &str = "exit_code";

/// Text in a check's output that marks a failure whatever the runner.
const FAILURE_MARKS: &[&str] = &[
    "Traceback (most recent call last)",
    "panicked at",
    "error[E",
    "--- FAIL",
    "error TS",
];

/// Words that count failures or errors after a number above zero (`2
/// failed`, `1 error`), in any case.
const FAILURE_COUNTS: &[&str] = &[
    "failed", "failure", "failures", "failing", "error", "errors",
];

/// A command's result, its JSON read once.
#[derive(Debug)]
pub(crate) struct Output {
    /// The result as the model was shown it: a text result as it is, a
    /// JSON one serialized
    pub shown: String,
    /// What the command printed, when that is not all of `shown`
    printed: Option<String>,
    /// Whether the result holds an exit code, which a failed call's has
    /// only when its command ran to its end
    pub exited: bool,
}

impl Output {
    /// The result that holds `content`.
    pub(crate) fn of(content: &ToolResultContent) -> Self {
        let shown = match content {
            ToolResultContent::Text(text) => text.clone(),
            ToolResultContent::Json(value) => value.to_string(),
        };
        let json = serde_json::from_str::<Value>(&shown).ok();
        let exited = match &json {
            Some(value) => value.get(EXIT_CODE).is_some_and(Value::is_i64),
            None => shown
                .trim_start()
                .strip_prefix(EXIT_CODE)
                .is_some_and(|rest| rest.starts_with(':')),
        };
        let printed = json.as_ref().and_then(Value::as_object).and_then(streams);
        Self {
            shown,
            printed,
            exited,
        }
    }

    /// What the command printed: the `stdout` and `stderr` of a JSON
    /// result, else the result as it is.
    pub(crate) fn printed(&self) -> &str {
        self.printed.as_deref().unwrap_or(&self.shown)
    }
}

/// The `stdout` and `stderr` of a JSON result's `fields` that are not
/// blank, on lines of their own; `None` when there are none.
fn streams(fields: &Map<String, Value>) -> Option<String> {
    let parts: Vec<&str> = ["stdout", "stderr"]
        .iter()
        .filter_map(|key| fields.get(*key).and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// Whether what a check printed shows a failed test or an error: a count
/// of failures or errors above zero ([`FAILURE_COUNTS`]), a word a runner
/// prints for one (`FAILED`, `FAIL`), an exception (`TypeError:`) or one
/// of [`FAILURE_MARKS`].
pub(crate) fn shows_failures(output: &str) -> bool {
    if FAILURE_MARKS.iter().any(|mark| output.contains(mark)) {
        return true;
    }
    let mut previous = "";
    for word in output.split_whitespace() {
        let bare = word.trim_matches(|c: char| !c.is_alphanumeric());
        let exception =
            word.ends_with(':') && (bare.ends_with("Error") || bare.ends_with("Exception"));
        if exception || matches!(bare, "FAILED" | "FAIL") {
            return true;
        }
        let counted = FAILURE_COUNTS
            .iter()
            .any(|count| bare.eq_ignore_ascii_case(count));
        if counted && previous.parse::<u32>().is_ok_and(|n| n > 0) {
            return true;
        }
        previous = bare;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_result_is_read_for_what_it_printed_and_whether_it_exited() {
        let read = |text: &str| Output::of(&ToolResultContent::Text(text.to_string()));
        let piped = read(r#"{"exit_code":1,"stdout":"2 failed","stderr":"warning: x"}"#);
        assert_eq!(piped.printed(), "2 failed\nwarning: x");
        assert!(piped.exited);
        let blank = read(r#"{"ok":false,"stdout":" ","stderr":"command not allowed"}"#);
        assert_eq!(blank.printed(), "command not allowed");
        assert!(!blank.exited, "no exit code");
        for text in [
            "exit_code: 2\nstderr:\nboom",
            "  exit_code:1",
            "denied",
            "7",
            "\"x\"",
        ] {
            let result = read(text);
            assert_eq!(result.printed(), text, "{text}");
            assert_eq!(result.exited, text.trim_start().starts_with("exit_code:"));
        }
        assert!(
            !read(r#"{"exit_code":"1"}"#).exited,
            "an exit code that is no number"
        );
        let json = Output::of(&ToolResultContent::Json(
            json!({"exit_code": 0, "stdout": "ok"}),
        ));
        assert_eq!(json.shown, r#"{"exit_code":0,"stdout":"ok"}"#);
        assert_eq!(json.printed(), "ok");
    }

    #[test]
    fn failures_are_read_from_what_a_check_printed() {
        for output in [
            "===== 2 failed, 9 passed in 0.3s =====",
            "collected 4 items / 1 error",
            "test result: FAILED. 3 passed; 1 failed; 0 ignored",
            "Tests:       1 failed, 5 passed, 6 total",
            "FAILED (failures=1)",
            "--- FAIL: TestParse (0.00s)",
            "E   TypeError: unsupported operand",
            "thread 'main' panicked at src/lib.rs:4:5",
            "error[E0308]: mismatched types",
            "src/a.ts(3,1): error TS2304: Cannot find name 'x'.",
            "2 FAILURES",
        ] {
            assert!(shows_failures(output), "{output}");
        }
        for output in [
            "===== 9 passed, 7 warnings in 0.08s =====",
            "test result: ok. 12 passed; 0 failed; 0 ignored",
            "Tests:       6 passed, 6 total",
            "ok  \texample.com/pkg\t0.004s",
            "test_raises_type_error PASSED",
            "0 errors, 0 warnings",
            "",
        ] {
            assert!(!shows_failures(output), "{output}");
        }
    }
}
