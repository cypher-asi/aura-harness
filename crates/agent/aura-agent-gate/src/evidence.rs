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

use crate::checks::{checks_placed, command_line, exit_status_masked};
use crate::followup::{gave_up, is_nudge, says_unverified};
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

/// Text in a command line that changes files in the workspace, as far as
/// a command line shows it.
const SHELL_WRITES: &[&str] = &[
    "sed -i",
    "perl -pi",
    "perl -i",
    "git apply",
    "git checkout ",
    "git restore ",
    "git stash",
    "patch -p",
    ".write(",
    "write_text(",
];

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
    /// The files it changed by any means, relative to the workspace,
    /// when the harness compared the workspace before and after: those
    /// changed by commands it ran are among them
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_paths: Option<Vec<String>>,
    /// Whether it changed a file after the last check it ran to its end,
    /// as far as its tool calls show
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub changed_after_check: bool,
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
        let seen = checks_placed(messages);
        let checks_run = u32::try_from(seen.len()).unwrap_or(u32::MAX);
        let last_check_at = seen.last().map(|(at, _)| *at);
        let checks = seen
            .into_iter()
            .rev()
            .take(KEPT_CHECKS)
            .rev()
            .map(|(_, check)| CheckEvidence {
                command: head(&check.command, KEPT_COMMAND),
                passed: check.passed,
                output: tail(&printed(&check.output), KEPT_TEXT),
            })
            .collect();
        let mut files_edited: Vec<String> = Vec::new();
        let (mut calls, mut commands, mut nudges) = (0, 0, 0);
        // The place of the tool call that last changed a file, among the
        // tool calls.
        let (mut tool_calls, mut last_change_at) = (0, None);
        for message in messages {
            match message.role {
                Role::Assistant => calls += 1,
                Role::User if is_nudge(&message.text_content()) => nudges += 1,
                Role::User => {}
            }
            for block in &message.content {
                let ContentBlock::ToolUse { name, input, .. } = block else {
                    continue;
                };
                let at = tool_calls;
                tool_calls += 1;
                if name == "run_command" {
                    commands += 1;
                    let command = command_line(input);
                    if SHELL_WRITES.iter().any(|mark| command.contains(mark)) {
                        last_change_at = Some(at);
                    }
                } else if FILE_TOOLS.contains(&name.as_str()) {
                    last_change_at = Some(at);
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
            changed_paths: None,
            changed_after_check: last_check_at
                .zip(last_change_at)
                .is_some_and(|(check, change)| change > check),
            hit_limit: None,
            reply: head(reply.trim(), KEPT_TEXT),
        }
    }

    /// Note the files the attempt changed by any means, as the harness
    /// found by comparing the workspace before and after.
    pub fn note_changed(&mut self, paths: &[String]) {
        self.files_changed = Some(u32::try_from(paths.len()).unwrap_or(u32::MAX));
        self.changed_paths = Some(paths.iter().take(KEPT_PATHS).cloned().collect());
    }

    /// The changed files its file tools did not change: `None` when the
    /// harness did not compare the workspace, or when more files were
    /// changed than are kept.
    #[must_use]
    pub fn changed_by_commands(&self) -> Option<Vec<&str>> {
        let changed = self.changed_paths.as_ref()?;
        if changed.len() >= KEPT_PATHS || self.files_edited.len() >= KEPT_PATHS {
            return None;
        }
        Some(
            changed
                .iter()
                .filter(|path| {
                    !self
                        .files_edited
                        .iter()
                        .any(|edited| same_file(edited, path))
                })
                .map(String::as_str)
                .collect(),
        )
    }

    /// The paths known of what it changed: every changed file when the
    /// harness compared the workspace, else those its file tools changed.
    fn known_paths(&self) -> &[String] {
        self.changed_paths.as_deref().unwrap_or(&self.files_edited)
    }

    /// The last check the attempt ran to its end.
    #[must_use]
    pub fn last_check(&self) -> Option<&CheckEvidence> {
        self.checks.last()
    }
}

/// What a command printed, from the result the model was shown: the
/// `stdout` and `stderr` of a JSON result, else the result as it is.
pub(crate) fn printed(result: &str) -> String {
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

/// Whether `a` and `b` name one file: equal, or one the other's path
/// from further up (an absolute path and one relative to the workspace).
fn same_file(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim_start_matches("./"), b.trim_start_matches("./"));
    let under = |long: &str, short: &str| {
        long.strip_suffix(short)
            .is_some_and(|rest| rest.is_empty() || rest.ends_with('/'))
    };
    under(a, b) || under(b, a)
}

/// Whether `path` looks like a test or its data.
#[must_use]
pub fn is_test_path(path: &str) -> bool {
    let path = path.to_lowercase();
    path.contains("test") || path.contains("spec")
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
    /// It changed files with commands it ran rather than with the file
    /// tools ([`Evidence::changed_by_commands`]).
    ShellEdit,
    /// It changed a file after the last check it ran.
    StaleCheck,
    /// Its reply says the tests it ran do not cover its change.
    ReplyNotCovered,
    /// Work that changes code added or changed no test.
    NoTestChanged,
}

impl Flag {
    /// Every flag, in the order reports list them.
    pub const ALL: [Self; 13] = [
        Self::MaskedCheck,
        Self::CheckOutputFailures,
        Self::MaskedCheckWithFailures,
        Self::ReplyUnverified,
        Self::ReplyFailures,
        Self::NoCheck,
        Self::NoChanges,
        Self::GaveUp,
        Self::HitLimit,
        Self::ShellEdit,
        Self::StaleCheck,
        Self::ReplyNotCovered,
        Self::NoTestChanged,
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
            Self::ShellEdit => "shell_edit",
            Self::StaleCheck => "stale_check",
            Self::ReplyNotCovered => "reply_not_covered",
            Self::NoTestChanged => "no_test_changed",
        }
    }

    /// The flag in words, for whoever reviews or repairs the work.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::MaskedCheck => "the last test or build ran through a pipe or a chain of commands, so its exit status is not the check's own",
            Self::CheckOutputFailures => "the last test or build exited with success, but what it printed shows failures or errors",
            Self::MaskedCheckWithFailures => "the last test or build printed failures or errors, and its exit status was another command's",
            Self::ReplyUnverified => "the final reply says the work was not verified or not tested",
            Self::ReplyFailures => "the final reply says tests still fail",
            Self::NoCheck => "no test or build was run to its end",
            Self::NoChanges => "the request asks for a code change, but no file was changed",
            Self::GaveUp => "the final reply says the work could not be done or was left unfinished",
            Self::HitLimit => "the attempt stopped on a limit (iterations, budget or a stall) instead of finishing",
            Self::ShellEdit => "files were changed by shell commands rather than the file tools",
            Self::StaleCheck => "a file was changed after the last test or build ran",
            Self::ReplyNotCovered => "the final reply says the tests that were run do not cover the change",
            Self::NoTestChanged => "no test was added or changed",
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
    if evidence
        .changed_by_commands()
        .is_some_and(|paths| !paths.is_empty())
    {
        found.push(Flag::ShellEdit);
    }
    if evidence.changed_after_check {
        found.push(Flag::StaleCheck);
    }
    if NOT_COVERED.iter().any(|phrase| reply.contains(phrase)) {
        found.push(Flag::ReplyNotCovered);
    }
    let paths = evidence.known_paths();
    if verifiable(category) && !paths.is_empty() && !paths.iter().any(|path| is_test_path(path)) {
        found.push(Flag::NoTestChanged);
    }
    found
}

/// Phrases of a reply that says the tests it ran do not cover the
/// change, lowercase.
const NOT_COVERED: &[&str] = &[
    "don't cover",
    "doesn't cover",
    "do not cover",
    "does not cover",
    "not covered by",
    "isn't covered",
    "aren't covered",
    "no unit tests exist",
    "no tests exist",
    "no test covers",
    "no tests cover",
    "nothing covers",
    "no existing test",
];

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
                Flag::NoTestChanged,
            ]
        );
    }

    #[test]
    fn a_clean_attempt_raises_no_flag() {
        let messages = [
            call("1", "edit_file", json!({"path": "src/lib.rs"})),
            result("1", "ok"),
            call("0", "edit_file", json!({"path": "tests/parser.rs"})),
            result("0", "ok"),
            call(
                "2",
                "run_command",
                json!({"command": "cargo test -p parser"}),
            ),
            result("2", "test result: ok. 12 passed; 0 failed; 0 ignored"),
            Message::assistant("Fixed the parser; `cargo test -p parser` passes (12 tests)."),
        ];
        let mut evidence = Evidence::of(&messages);
        evidence.note_changed(&["src/lib.rs".to_string(), "tests/parser.rs".to_string()]);
        evidence.hit_limit = Some(false);
        assert_eq!(flags(&evidence, PromptCategory::SmallChange), []);
    }

    #[test]
    fn how_the_change_was_made_and_checked_is_flagged() {
        let messages = [
            call("1", "edit_file", json!({"path": "/work/copy/src/lib.rs"})),
            result("1", "ok"),
            call(
                "2",
                "run_command",
                json!({"command": "cargo test -p parser"}),
            ),
            result("2", "test result: ok. 12 passed; 0 failed"),
            call(
                "3",
                "run_command",
                json!({"command": "sed -i 's/a/b/' src/parse.rs"}),
            ),
            result("3", ""),
            Message::assistant(
                "Done. No tests exist for the new path, so the suite doesn't cover it.",
            ),
        ];
        let mut evidence = Evidence::of(&messages);
        assert!(evidence.changed_after_check, "sed ran after the tests");
        assert_eq!(evidence.changed_by_commands(), None, "nothing compared");
        assert_eq!(
            flags(&evidence, PromptCategory::SmallChange),
            [Flag::StaleCheck, Flag::ReplyNotCovered, Flag::NoTestChanged]
        );
        evidence.note_changed(&["src/lib.rs".to_string(), "src/parse.rs".to_string()]);
        assert_eq!(evidence.files_changed, Some(2));
        assert_eq!(evidence.changed_by_commands(), Some(vec!["src/parse.rs"]));
        assert_eq!(
            flags(&evidence, PromptCategory::SmallChange),
            [
                Flag::ShellEdit,
                Flag::StaleCheck,
                Flag::ReplyNotCovered,
                Flag::NoTestChanged
            ]
        );
        let back: Evidence =
            serde_json::from_str(&serde_json::to_string(&evidence).unwrap()).unwrap();
        assert_eq!(back, evidence);

        let checked_last = [
            call(
                "1",
                "run_command",
                json!({"command": "sed -i 's/a/b/' src/parse.rs"}),
            ),
            result("1", ""),
            call("2", "run_command", json!({"command": "cargo test"})),
            result("2", "ok"),
        ];
        assert!(!Evidence::of(&checked_last).changed_after_check);
        assert!(same_file("/work/copy/src/lib.rs", "src/lib.rs"));
        assert!(same_file("./src/lib.rs", "src/lib.rs"));
        assert!(!same_file("src/mylib.rs", "lib.rs"));
        assert!(Flag::ALL.iter().all(|flag| !flag.describe().is_empty()));
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
