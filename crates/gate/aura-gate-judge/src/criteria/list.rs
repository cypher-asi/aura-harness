//! The criteria a request's work is held to, and what the worker is told
//! of them.

use serde::{Deserialize, Serialize};

use super::{Kind, EXACTING_ITEMS};

/// One thing the finished work must do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Criterion {
    /// What, as the request says it
    pub text: String,
    pub kind: Kind,
    /// What the work must match, when the request names it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// What a request's work will be checked against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Criteria {
    /// At most `MAX_ITEMS`, in the writer's order
    pub items: Vec<Criterion>,
    /// Whether the request's output must match a format or a reference
    /// exactly
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strict: bool,
}

impl Criteria {
    /// Whether work held to these is hard to get right however plain its
    /// request reads: its output must match exactly, as the writer said
    /// or as `EXACTING_ITEMS` or more criteria say.
    #[must_use]
    pub fn exacting(&self) -> bool {
        self.strict || self.items.iter().filter(|c| c.kind.exacting()).count() >= EXACTING_ITEMS
    }

    /// The criteria as a numbered list, one per line, e.g.
    /// `1. [exact output] ... (reference: ...)`.
    #[must_use]
    pub(crate) fn listed(&self) -> String {
        self.items
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let reference = c
                    .reference
                    .as_deref()
                    .map_or_else(String::new, |r| format!(" (reference: {r})"));
                format!("{}. [{}] {}{reference}", i + 1, c.kind.label(), c.text)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// What the worker is told after its request: what the work will be
/// checked against.
#[must_use]
pub fn worker_note(criteria: &Criteria) -> String {
    format!(
        "Before you started, the harness wrote down what this work will be checked against. \
         Before you finish, make sure each holds and that a test or command you ran shows it. \
         Where one names a reference, compare against that reference itself on every path the \
         change adds, rather than against output written out by hand.\n\n{}",
        criteria.listed()
    )
}
