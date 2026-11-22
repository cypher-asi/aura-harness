//! A piece of work Gate routes: its level, what it asks for, how hard it
//! is and, for a subtask, the permanent agent it goes to.

use aura_core_types::{Difficulty, PromptCategory, RouteLevel};

/// A piece of work Gate routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Work<'a> {
    /// Turn or subtask.
    pub level: RouteLevel,
    /// What it asks for.
    pub category: PromptCategory,
    /// How hard the classifier judged it; `None` when it did not say.
    pub difficulty: Option<Difficulty>,
    /// The permanent agent a subtask goes to.
    pub kind: Option<&'a str>,
}

impl<'a> Work<'a> {
    /// Work at `level` asking for `category`, for the permanent agent
    /// `kind` when it is a subtask, of unknown difficulty.
    #[must_use]
    pub const fn new(level: RouteLevel, category: PromptCategory, kind: Option<&'a str>) -> Self {
        Self {
            level,
            category,
            difficulty: None,
            kind,
        }
    }

    /// A turn asking for `category`, of unknown difficulty.
    #[must_use]
    pub const fn turn(category: PromptCategory) -> Self {
        Self::new(RouteLevel::Turn, category, None)
    }

    /// The same work, of `difficulty`.
    #[must_use]
    pub const fn of(self, difficulty: Option<Difficulty>) -> Self {
        Self { difficulty, ..self }
    }
}
