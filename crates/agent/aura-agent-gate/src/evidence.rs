//! What an attempt left behind that says how it went, before anything
//! outside it checks the work: the checks it ran and what they printed,
//! what it changed, and what it said at the end.
//!
//! [`Evidence`] is the raw material, kept in Gate's attempt log
//! ([`crate::attempt_log`]). [`flags`] reads signs of a failed attempt
//! out of it. The flags are computed when the log is evaluated, not when
//! it is written, so a new one is scored on every attempt already
//! logged.

use aura_core_types::PromptCategory;
use aura_model::{ContentBlock, Message, Role};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::checks::{checks_seen, exit_status_masked};
use crate::followup::{gave_up, says_unverified, NUDGE_PROMPT};
use crate::router::verifiable;

/// Checks kept with their output, latest last.
const KEPT_CHECKS: usize = 4;

/// Characters kept of a check's output (its end) and of the reply (its
/// start).
const KEPT_TEXT: usize = 4000;

/// Characters kept of a check's command line.
const KEPT_COMMAND: usize = 600;

/// Paths kept of the files the attempt's file tools changed.
const KEPT_PATHS: usize = 40;

/// Tools that change a file.
const FILE_TOOLS: [&str; 3] = ["write_file", "edit_file", "delete_file"];

/// A check the attempt ran to its end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckEvidence {
    /// Its command line
    pub command: String,
    /// Whether the command exited with success, which is what Gate counts
    /// as the check passing
    pub passed: bool,
    /// The end of what it printed
    pub output: String,
}

/// What one attempt left behind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// Model calls of the attempt
    pub calls: u32,
    /// Commands it ran
    pub commands: u32,
    /// Checks it ran to their end
    pub checks_run: u32,
    /// Times Gate asked it to check a change it had not checked
    #[serde(default, skip_serializing_if = "is_zero")]
    pub nudges: u32,
    /// The last of those checks, latest last
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CheckEvidence>,
    /// Files its file tools changed. A file changed by a command it ran
    /// is not among them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_edited: Vec<String>,
    /// Files it changed, when the harness counted them
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_changed: Option<u32>,
    /// Whether it stopped on a limit rather than by finishing, when known
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_limit: Option<bool>,
    /// The start of its final reply
    pub reply: String,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Evidence {
    /// The evidence in `messages`, an attempt's own.
    #[must_use]
    pub fn of(messages: &[Message]) -> Self {
        let seen = checks_seen(messages);
        let checks_run = u32::try_from(seen.len()).unwrap_or(u32::MAX);
        let checks = seen
            .into_iter()
            .rev()
            .take(KEPT_CHECKS)
            .rev()
            .map(|check| CheckEvidence {
                command: head(&check.command, KEPT_COMMAND),
                passed: check.passed,
                output: tail(&printed(&check.output), KEPT_TEXT),
            })
            .collect();
        let mut files_edited: Vec<String> = Vec::new();
        let (mut calls, mut commands, mut nudges) = (0, 0, 0);
        for message in messages {
            match message.role {
                Role::Assistant => calls += 1,
                Role::User if message.text_content().contains(NUDGE_PROMPT) => nudges += 1,
                Role::User => {}
            }
            for block in &message.content {
                let ContentBlock::ToolUse { name, input, .. } = block else {
                    continue;
                };
                if name == "run_command" {
                    commands += 1;
                } else if FILE_TOOLS.contains(&name.as_str()) {
                    let path = input["path"].as_str().unwrap_or_default().to_string();
                    if !path.is_empty()
                        && !files_edited.contains(&path)
                        && files_edited.len() < KEPT_PATHS
                    {
                        files_edited.push(path);
                    }
                }
            }
        }
        let reply = messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant && !m.text_content().trim().is_empty())
            .map(Message::text_content)
            .unwrap_or_default();
        Self {
            calls,
            commands,
            checks_run,
            nudges,
            checks,
            files_edited,
            files_changed: None,
            hit_limit: None,
            reply: head(reply.trim(), KEPT_TEXT),
        }
    }

    /// The last check the attempt ran to its end.
    #[must_use]
    pub fn last_check(&self) -> Option<&CheckEvidence> {
        self.checks.last()
    }
}

/// What a command printed, from the result the model was shown: the
/// `stdout` and `stderr` of a JSON result, else the result as it is.
fn printed(result: &str) -> String {
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(result) else {
        return result.to_string();
    };
    let parts: Vec<&str> = ["stdout", "stderr"]
        .iter()
        .filter_map(|key| fields.get(*key).and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .collect();
    if parts.is_empty() {
        result.to_string()
    } else {
        parts.join("\n")
    }
}

fn head(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}

fn tail(text: &str, chars: usize) -> String {
    let skip = text.chars().count().saturating_sub(chars);
    text.chars().skip(skip).collect()
}

/// A sign in an attempt's [`Evidence`] that it may have failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    /// Its last check counted as passed, but the command's exit status
    /// was not the check's ([`exit_status_masked`]).
    MaskedCheck,
    /// Its last check counted as passed, but what it printed shows
    /// failures or errors.
    CheckOutputFailures,
    /// Both of the above: nothing says the check passed, and its output
    /// says it did not.
    MaskedCheckWithFailures,
    /// Its reply says the work was not verified
    /// ([`says_unverified`]). After a check that counted as passed, Gate
    /// has the judge review such an attempt (see [`crate::followup`]).
    ReplyUnverified,
    /// Its reply says tests still fail.
    ReplyFailures,
    /// It ran no check to its end.
    NoCheck,
    /// Work that changes code changed no file.
    NoChanges,
    /// Its reply says it could not do the work.
    GaveUp,
    /// It stopped on a limit.
    HitLimit,
}

impl Flag {
    /// Every flag, in the order reports list them.
    pub const ALL: [Self; 9] = [
        Self::MaskedCheck,
        Self::CheckOutputFailures,
        Self::MaskedCheckWithFailures,
        Self::ReplyUnverified,
        Self::ReplyFailures,
        Self::NoCheck,
        Self::NoChanges,
        Self::GaveUp,
        Self::HitLimit,
    ];

    /// The flag's name as logged, e.g. `masked_check`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::MaskedCheck => "masked_check",
            Self::CheckOutputFailures => "check_output_failures",
            Self::MaskedCheckWithFailures => "masked_check_with_failures",
            Self::ReplyUnverified => "reply_unverified",
            Self::ReplyFailures => "reply_failures",
            Self::NoCheck => "no_check",
            Self::NoChanges => "no_changes",
            Self::GaveUp => "gave_up",
            Self::HitLimit => "hit_limit",
        }
    }
}

/// The flags `evidence` raises for an attempt at work asking for
/// `category`.
#[must_use]
pub fn flags(evidence: &Evidence, category: PromptCategory) -> Vec<Flag> {
    let mut found = Vec::new();
    match evidence.last_check() {
        Some(check) if check.passed => {
            let masked = exit_status_masked(&check.command);
            let failures = shows_failures(&check.output);
            if masked {
                found.push(Flag::MaskedCheck);
            }
            if failures {
                found.push(Flag::CheckOutputFailures);
            }
            if masked && failures {
                found.push(Flag::MaskedCheckWithFailures);
            }
        }
        Some(_) => {}
        None => found.push(Flag::NoCheck),
    }
    if says_unverified(&evidence.reply) {
        found.push(Flag::ReplyUnverified);
    }
    let reply = evidence.reply.to_lowercase().replace('’', "'");
    if STILL_FAILING.iter().any(|phrase| reply.contains(phrase)) {
        found.push(Flag::ReplyFailures);
    }
    if verifiable(category) && evidence.files_changed == Some(0) {
        found.push(Flag::NoChanges);
    }
    if gave_up(&evidence.reply) {
        found.push(Flag::GaveUp);
    }
    if evidence.hit_limit == Some(true) {
        found.push(Flag::HitLimit);
    }
    found
}

/// Phrases of a reply that says tests still fail, lowercase.
const STILL_FAILING: &[&str] = &[
    "still fail",
    "tests fail",
    "test fails",
    "is failing",
    "are failing",
    "doesn't pass",
    "does not pass",
    "don't pass",
    "do not pass",
];

/// Text in a check's output that marks a failure whatever the runner.
const FAILURE_MARKS: &[&str] = &[
    "Traceback (most recent call last)",
    "panicked at",
    "error[E",
    "--- FAIL",
    "error TS",
];

/// Whether what a check printed shows a failed test or an error: a count
/// of failures or errors above zero (`2 failed`, `1 error`), a word a
/// runner prints for one (`FAILED`, `FAIL`), an exception (`TypeError:`)
/// or one of [`FAILURE_MARKS`].
#[must_use]
pub fn shows_failures(output: &str) -> bool {
    if FAILURE_MARKS.iter().any(|mark| output.contains(mark)) {
        return true;
    }
    let trim = |word: &str| -> String {
        word.trim_matches(|c: char| !c.is_alphanumeric())
            .to_string()
    };
    let mut previous = String::new();
    for word in output.split_whitespace() {
        let bare = trim(word);
        if matches!(bare.as_str(), "FAILED" | "FAIL") {
            return true;
        }
        if word.ends_with(':') && (bare.ends_with("Error") || bare.ends_with("Exception")) {
            return true;
        }
        let counted = matches!(
            bare.to_lowercase().as_str(),
            "failed" | "failure" | "failures" | "failing" | "error" | "errors"
        );
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
    use aura_model::ToolResultContent;
    use serde_json::json;

    fn call(id: &str, name: &str, input: Value) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input,
            }],
        )
    }

    fn result(id: &str, content: &str) -> Message {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: ToolResultContent::Text(content.to_string()),
                is_error: false,
            }],
        )
    }

    /// The end of run a-4 of Compare #22: Claude Sonnet 5.5 on
    /// `ansible-d33bedc4`, which Gate let stand and the task's own check
    /// then failed.
    fn a4() -> Vec<Message> {
        vec![
            Message::user("Your task is to implement the changes necessary"),
            call("1", "edit_file", json!({"path": "lib/ansible/module_utils/parsing/convert_bool.py"})),
            result("1", "Replaced 1 occurrence(s)"),
            call(
                "2",
                "run_command",
                json!({"program": "sh", "args": [".swe/run", "python -m pytest -q -p no:cacheprovider test/units/config test/units/module_utils/parsing 2>&1 | grep -E 'passed|failed|FAILED|Error' | head -20"]}),
            ),
            result(
                "2",
                r#"{"exit_code":0,"ok":true,"stderr":"","stdout":"TypeError: TestFindIniFile.test_no_cwd_cfg_no_warning_on_writable.<locals>._os_stat() got an unexpected keyword argument 'follow_symlinks'\nAssertionError: assert '/usr/local/lib/python3.11/site-packages/_pytest/main.py' == '/app/test/units/config/manager'\n======================== 9 passed, 7 warnings in 0.08s =========================\n"}"#,
            ),
            Message::assistant(
                "I rewrote `ensure_type()` in `lib/ansible/config/manager.py` to follow the PR, but it isn't verified. The only test run I got was 9 tests passing, and that run was cut short by an unrelated test error.",
            ),
        ]
    }

    #[test]
    fn the_evidence_of_an_attempt_is_read_from_its_messages() {
        let evidence = Evidence::of(&a4());
        assert_eq!(
            (evidence.calls, evidence.commands, evidence.checks_run),
            (3, 1, 1)
        );
        assert_eq!(
            evidence.files_edited,
            ["lib/ansible/module_utils/parsing/convert_bool.py"]
        );
        let check = evidence.last_check().unwrap();
        assert!(check.passed, "the pipe exited 0");
        assert!(check.command.starts_with("sh .swe/run python -m pytest"));
        assert!(
            check.output.starts_with("TypeError: TestFindIniFile"),
            "{}",
            check.output
        );
        assert!(check.output.contains("9 passed"));
        assert!(evidence.reply.starts_with("I rewrote `ensure_type()`"));
        let back: Evidence =
            serde_json::from_str(&serde_json::to_string(&evidence).unwrap()).unwrap();
        assert_eq!(back, evidence);
    }

    #[test]
    fn a_passing_check_that_proves_nothing_is_flagged() {
        let evidence = Evidence::of(&a4());
        assert_eq!(
            flags(&evidence, PromptCategory::MultiFileChange),
            [
                Flag::MaskedCheck,
                Flag::CheckOutputFailures,
                Flag::MaskedCheckWithFailures,
                Flag::ReplyUnverified,
            ]
        );
    }

    #[test]
    fn a_clean_attempt_raises_no_flag() {
        let messages = [
            call("1", "edit_file", json!({"path": "src/lib.rs"})),
            result("1", "ok"),
            call(
                "2",
                "run_command",
                json!({"command": "cargo test -p parser"}),
            ),
            result("2", "test result: ok. 12 passed; 0 failed; 0 ignored"),
            Message::assistant("Fixed the parser; `cargo test -p parser` passes (12 tests)."),
        ];
        let mut evidence = Evidence::of(&messages);
        evidence.files_changed = Some(1);
        evidence.hit_limit = Some(false);
        assert_eq!(flags(&evidence, PromptCategory::SmallChange), []);
    }

    #[test]
    fn the_other_signs_are_flagged() {
        let mut evidence = Evidence::of(&[Message::assistant(
            "I was unable to find the cause. Two tests still fail.",
        )]);
        evidence.files_changed = Some(0);
        evidence.hit_limit = Some(true);
        assert_eq!(
            flags(&evidence, PromptCategory::Debugging),
            [
                Flag::NoCheck,
                Flag::ReplyFailures,
                Flag::NoChanges,
                Flag::GaveUp,
                Flag::HitLimit
            ]
        );
        assert_eq!(
            flags(&evidence, PromptCategory::Question),
            [
                Flag::NoCheck,
                Flag::ReplyFailures,
                Flag::GaveUp,
                Flag::HitLimit
            ],
            "a question changes no file"
        );
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

    #[test]
    fn long_output_keeps_its_end_and_a_long_reply_its_start() {
        let long = format!("{}END", "x".repeat(3 * KEPT_TEXT));
        let messages = [
            call("1", "run_command", json!({"command": "pytest"})),
            result("1", &long),
            Message::assistant(format!("START{}", "y".repeat(3 * KEPT_TEXT))),
        ];
        let evidence = Evidence::of(&messages);
        let check = evidence.last_check().unwrap();
        assert_eq!(check.output.chars().count(), KEPT_TEXT);
        assert!(check.output.ends_with("END"));
        assert_eq!(evidence.reply.chars().count(), KEPT_TEXT);
        assert!(evidence.reply.starts_with("START"));
    }
}
