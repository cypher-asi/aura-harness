//! The parts of a turn its judges are shown, each cut to its limit and
//! named when it is blank.

use std::borrow::Cow;

use crate::blind::MAX_REPLY;
use crate::request::{cut, or_blank, tail};

/// Longest request shown to a judge, in characters.
const MAX_TASK: usize = 8_000;

/// Longest account of a failure shown to a judge, in characters, and of
/// the commands a turn ran or the checks it ran last; the end of a test
/// run says what failed.
pub(super) const MAX_FAILURE: usize = 3_000;

/// What the user asked.
pub(super) fn task(task: &str) -> Cow<'_, str> {
    cut(task, MAX_TASK, "request")
}

/// The turn's file changes, at most `max` characters of them.
pub(super) fn changes(diff: &str, max: usize) -> Cow<'_, str> {
    or_blank(diff, "(no file changes recorded)", |diff| {
        cut(diff, max, "diff")
    })
}

/// The commands the turn ran, the latest kept.
pub(super) fn commands(commands: &str) -> Cow<'_, str> {
    or_blank(commands, "(none)", |commands| tail(commands, MAX_FAILURE))
}

/// The checks the turn ran last and what they printed, the last `max`
/// characters of them.
pub(super) fn checks(checks: &str, max: usize) -> Cow<'_, str> {
    or_blank(checks, "(it ran none)", |checks| tail(checks, max))
}

/// The turn's final reply.
pub(super) fn reply(reply: &str) -> Cow<'_, str> {
    or_blank(reply, "(no reply)", |reply| cut(reply, MAX_REPLY, "reply"))
}

/// `items` as a list, one `- item` line each.
pub(super) fn bullets<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    items
        .into_iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}
