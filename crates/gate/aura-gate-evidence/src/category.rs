//! Which kinds of work a check verifies.

use aura_core_types::PromptCategory;

/// Whether a check verifies work asking for `category`: code changes and
/// debugging end in a test or build that passes or fails, and a failed
/// attempt goes on with a stronger model. So such work can start on a
/// cheaper model at little risk to the result.
#[must_use]
pub const fn verifiable(category: PromptCategory) -> bool {
    matches!(
        category,
        PromptCategory::SmallChange | PromptCategory::MultiFileChange | PromptCategory::Debugging
    )
}
