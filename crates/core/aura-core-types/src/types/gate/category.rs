//! What a prompt asks for and how hard its work looks, as Gate's
//! classifier labels it.

use serde::{Deserialize, Serialize};

/// What a prompt asks for, as Gate's classifier labels it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCategory {
    /// A quick question or explanation.
    Question,
    /// A small code change in one place.
    SmallChange,
    /// A change across several files.
    MultiFileChange,
    /// Fixing a failure.
    Debugging,
    /// Investigating a codebase or topic.
    Research,
    /// Designing or planning work.
    Planning,
    /// Prose, docs or messages.
    Writing,
    /// Anything else.
    Other,
}

impl PromptCategory {
    /// Every category, the ones the classifier picks from.
    pub const CLASSES: [Self; 8] = [
        Self::Question,
        Self::SmallChange,
        Self::MultiFileChange,
        Self::Debugging,
        Self::Research,
        Self::Planning,
        Self::Writing,
        Self::Other,
    ];

    /// The category's name as stored, e.g. `small_change`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::SmallChange => "small_change",
            Self::MultiFileChange => "multi_file_change",
            Self::Debugging => "debugging",
            Self::Research => "research",
            Self::Planning => "planning",
            Self::Writing => "writing",
            Self::Other => "other",
        }
    }

    /// The category named `name` (as [`Self::as_str`] spells it).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::CLASSES.into_iter().find(|c| c.as_str() == name)
    }
}

/// A stored category: `None` for a prompt nothing classified, which is
/// stored without one, or as `unclassified` by older records.
pub(super) fn stored_category<'de, D>(deserializer: D) -> Result<Option<PromptCategory>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let name: Option<String> = Option::deserialize(deserializer)?;
    match name.as_deref() {
        None | Some("unclassified") => Ok(None),
        Some(name) => PromptCategory::from_name(name)
            .map(Some)
            .ok_or_else(|| serde::de::Error::unknown_variant(name, &[])),
    }
}

impl std::fmt::Display for PromptCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How hard a prompt's work looks to Gate's classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    /// Routine work a fast model does reliably.
    Easy,
    /// Ordinary work.
    Medium,
    /// Subtle or wide-reaching work that weaker models often get wrong.
    Hard,
}

impl Difficulty {
    /// Every difficulty, easiest first.
    pub const ALL: [Self; 3] = [Self::Easy, Self::Medium, Self::Hard];

    /// The difficulty's name as stored, e.g. `easy`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Easy => "easy",
            Self::Medium => "medium",
            Self::Hard => "hard",
        }
    }

    /// The difficulty named `name` (as [`Self::as_str`] spells it).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.as_str() == name)
    }
}

impl std::fmt::Display for Difficulty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
