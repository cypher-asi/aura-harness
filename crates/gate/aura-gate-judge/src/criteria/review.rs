//! How the reviewer of work about to stand is asked about each
//! criterion, and what it found.

use serde::{Deserialize, Serialize};

use super::{Criteria, Criterion};

/// How the finished work stands against one criterion, as the reviewer
/// says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The diff does it, and a test or command the attempt ran shows it
    Met,
    /// The diff does not do it, or does it differently
    Unmet,
    /// The diff may do it, but nothing the attempt ran shows that it does
    Untested,
}

impl Status {
    /// The status as the reviewer writes it, e.g. `UNMET`.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Met => "MET",
            Self::Unmet => "UNMET",
            Self::Untested => "UNTESTED",
        }
    }

    fn from_word(word: &str) -> Option<Self> {
        [Self::Met, Self::Unmet, Self::Untested]
            .into_iter()
            .find(|status| status.word().eq_ignore_ascii_case(word))
    }
}

/// A criterion and how the reviewer found the work stands against it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checked {
    pub criterion: Criterion,
    /// `None` when the reviewer did not say
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
}

/// What the reviewer is told of `criteria`, after the request: the list,
/// and the line it gives each.
#[must_use]
pub(crate) fn review_section(criteria: &Criteria) -> String {
    format!(
        "## Acceptance criteria (written down before the work started)\n\n{}\n\n\
         Go through these one at a time. Before the findings, give each its own line (the one \
         place to write up what is met), \
         `CRITERION <number>: MET`, `CRITERION <number>: UNMET` or `CRITERION <number>: UNTESTED`, \
         then a few words: MET when the diff does it and a test or command the attempt ran shows \
         it, naming which; UNMET when the diff does not do it or does it differently; UNTESTED \
         when nothing the attempt ran shows it. A test that compares against output written out \
         by hand does not show a criterion that names a reference. An UNMET criterion is a \
         finding, and so is an UNTESTED one of kind exact output or matches a reference.",
        criteria.listed()
    )
}

/// Each of `criteria` with the status the reviewer's `findings` give it
/// on a `CRITERION <n>: <status>` line; the first such line for a number
/// that names a status counts.
#[must_use]
pub fn checked(criteria: &Criteria, findings: &str) -> Vec<Checked> {
    let mut statuses: Vec<Option<Status>> = vec![None; criteria.items.len()];
    for (number, status) in findings.lines().filter_map(criterion_line) {
        let slot = number.checked_sub(1).and_then(|i| statuses.get_mut(i));
        if let Some(slot) = slot.filter(|slot| slot.is_none()) {
            *slot = status;
        }
    }
    criteria
        .items
        .iter()
        .cloned()
        .zip(statuses)
        .map(|(criterion, status)| Checked { criterion, status })
        .collect()
}

/// The number and the status of `line` when it is a criterion's line,
/// `CRITERION <n>: <status>` in any case, after any list or bold
/// markers; the status is `None` when its word names none.
fn criterion_line(line: &str) -> Option<(usize, Option<Status>)> {
    let line = line.trim().trim_start_matches(['-', '*', ' ']);
    let (head, rest) = line.split_once(':')?;
    let mut words = head.split_whitespace();
    let (Some(label), Some(number), None) = (words.next(), words.next(), words.next()) else {
        return None;
    };
    if !label.eq_ignore_ascii_case("criterion") {
        return None;
    }
    let number = number.trim_start_matches('#').parse().ok()?;
    let word = rest
        .trim_start()
        .split(|c: char| !c.is_ascii_alphabetic())
        .next()
        .unwrap_or_default();
    Some((number, Status::from_word(word)))
}
