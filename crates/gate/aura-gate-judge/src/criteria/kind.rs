//! What a criterion asks for.

use serde::{Deserialize, Serialize};

/// What each kind means, as the system prompt and the tool say it.
pub(super) const KIND_MEANINGS: [(Kind, &str); 5] = [
    (
        Kind::ExactOutput,
        "output that must match a stated format byte for byte or exactly",
    ),
    (
        Kind::ReferenceMatch,
        "output or behaviour that must match a named reference implementation, file or standard",
    ),
    (
        Kind::Error,
        "the error or exception a given input must raise",
    ),
    (
        Kind::Interface,
        "a name, signature, import path or type that must exist",
    ),
    (Kind::Behaviour, "anything else the work must do"),
];

/// What a criterion asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Output that must match a stated format exactly
    ExactOutput,
    /// Output or behaviour that must match a named reference
    ReferenceMatch,
    /// The error a given input must raise
    Error,
    /// A name, signature, import path or type that must exist
    Interface,
    /// Anything else the work must do
    Behaviour,
}

impl Kind {
    /// Every kind, in the order the writer is told them.
    pub(crate) const ALL: [Self; 5] = [
        Self::ExactOutput,
        Self::ReferenceMatch,
        Self::Error,
        Self::Interface,
        Self::Behaviour,
    ];

    /// The kind's name as stored, e.g. `exact_output`.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ExactOutput => "exact_output",
            Self::ReferenceMatch => "reference_match",
            Self::Error => "error",
            Self::Interface => "interface",
            Self::Behaviour => "behaviour",
        }
    }

    /// The kind named `name`, as [`Self::as_str`] spells it, or as a
    /// writer may: `behavior`, `exact-output`.
    #[must_use]
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        let name = name.replace(['-', ' '], "_");
        let name = if name == "behavior" {
            "behaviour"
        } else {
            name.as_str()
        };
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    /// The kind in words, for the worker and the reviewer, e.g.
    /// `exact output`.
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::ExactOutput => "exact output",
            Self::ReferenceMatch => "matches a reference",
            Self::Error => "error",
            Self::Interface => "interface",
            Self::Behaviour => "behaviour",
        }
    }

    /// Whether work is held to output that must match something exactly,
    /// which a test of the worker's own easily gets wrong.
    #[must_use]
    pub(crate) const fn exacting(self) -> bool {
        matches!(self, Self::ExactOutput | Self::ReferenceMatch)
    }
}
