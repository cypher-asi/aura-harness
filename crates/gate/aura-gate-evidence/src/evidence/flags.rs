//! The signs of a failed attempt that its evidence raises.

use aura_core_types::PromptCategory;

use super::types::same_file;
use super::{Evidence, Flag};
use crate::checks::exit_status_masked;
use crate::transcript::reply::Reply;
use crate::transcript::shows_failures;
use crate::verifiable;

/// The flags `evidence` raises for an attempt at work asking for
/// `category`.
#[must_use]
pub fn flags(evidence: &Evidence, category: PromptCategory) -> Vec<Flag> {
    let reply = Reply::read(&evidence.reply);
    let code = verifiable(category);
    let last = evidence.last_check();
    let passed = last.filter(|check| check.passed);
    let masked = passed.is_some_and(|check| exit_status_masked(&check.command));
    let failures = passed.is_some_and(|check| shows_failures(&check.output));
    let shell_edit = evidence
        .changed_by_commands()
        .is_some_and(|paths| !paths.is_empty());
    let paths = evidence.known_paths();
    let no_test = code && !paths.is_empty() && !paths.iter().any(|path| is_test_path(path));
    let own_tests_only =
        passed.is_some_and(|check| runs_only_tests_in(&check.command, &evidence.files_written));
    [
        (Flag::MaskedCheck, masked),
        (Flag::CheckOutputFailures, failures),
        (Flag::MaskedCheckWithFailures, masked && failures),
        (Flag::NoCheck, last.is_none()),
        (Flag::ReplyUnverified, reply.says_unverified()),
        (Flag::ReplyFailures, reply.says_still_failing()),
        (Flag::NoChanges, code && evidence.files_changed == Some(0)),
        (Flag::GaveUp, reply.gave_up()),
        (Flag::HitLimit, evidence.hit_limit == Some(true)),
        (Flag::ShellEdit, shell_edit),
        (Flag::StaleCheck, evidence.changed_after_check),
        (Flag::ReplyNotCovered, reply.says_not_covered()),
        (Flag::NoTestChanged, no_test),
        (Flag::OwnTestsOnly, own_tests_only),
    ]
    .into_iter()
    .filter_map(|(flag, raised)| raised.then_some(flag))
    .collect()
}

/// Whether `path` looks like a test or its data.
#[must_use]
pub fn is_test_path(path: &str) -> bool {
    let path = path.to_lowercase();
    path.contains("test") || path.contains("spec")
}

/// Whether `command` names tests to run, and only files among `written`:
/// a directory of tests runs others too.
fn runs_only_tests_in(command: &str, written: &[String]) -> bool {
    let named = tests_named(command);
    !named.is_empty()
        && named
            .iter()
            .all(|path| written.iter().any(|file| same_file(file, path)))
}

/// The tests `command` names by path, as a test runner is handed them:
/// test files (`test/units/test_urls.py`, less a selector of tests in it
/// such as `::test_parse`) and directories of tests (`test/units/`).
pub(super) fn tests_named(command: &str) -> Vec<&str> {
    command
        .split_whitespace()
        .map(|word| word.trim_matches(['\'', '"', '`', '(', ')', ';']))
        .map(|word| word.split("::").next().unwrap_or(word))
        .filter(|word| !word.starts_with('-') && !word.contains('='))
        .filter(|word| word.contains('/') || word.contains('.'))
        .filter(|word| is_test_path(word))
        .collect()
}
