//! The rows of the new-benchmark form, and what each names.

use crate::app::PickerTarget;

/// Rows of the new-benchmark form, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormField {
    /// The top-level agent the benchmark is run from
    From,
    #[default]
    Work,
    Prompt,
    Tasks,
    A,
    /// The models of arm A's easy, medium and hard work, when it runs work
    /// by its difficulty
    AEasy,
    AMedium,
    AHard,
    B,
    BEasy,
    BMedium,
    BHard,
    /// The model that classifies each run's prompt; [`AUTO_CLASSIFIER`](super::AUTO_CLASSIFIER)
    /// for Gate's choice
    Classifier,
    Repeats,
    Check,
    Budget,
    Go,
}

impl FormField {
    pub const ALL: [Self; 17] = [
        Self::From,
        Self::Work,
        Self::Prompt,
        Self::Tasks,
        Self::A,
        Self::AEasy,
        Self::AMedium,
        Self::AHard,
        Self::B,
        Self::BEasy,
        Self::BMedium,
        Self::BHard,
        Self::Classifier,
        Self::Repeats,
        Self::Check,
        Self::Budget,
        Self::Go,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::From => "From",
            Self::Work => "Work",
            Self::Prompt => "Prompt",
            Self::Tasks => "Tasks",
            Self::A => "Arm A",
            Self::B => "Arm B",
            Self::AEasy | Self::BEasy => "  Easy",
            Self::AMedium | Self::BMedium => "  Medium",
            Self::AHard | Self::BHard => "  Hard",
            Self::Classifier => "Classifier",
            Self::Repeats => "Runs per arm",
            Self::Check => "Check",
            Self::Budget => "Budget (USD)",
            Self::Go => "",
        }
    }

    /// Whether its value is typed in the input line.
    #[must_use]
    pub const fn is_text(self) -> bool {
        matches!(self, Self::Prompt | Self::Check | Self::Budget) || self.holds_model()
    }

    /// Whether the row holds a model, picked in the `/model` selector:
    /// a difficulty's row, or the classifier's.
    #[must_use]
    pub const fn holds_model(self) -> bool {
        self.picker_target().is_some()
    }

    /// What the model selector picks for, opened on this row; `None` for
    /// a row that holds no model.
    #[must_use]
    pub const fn picker_target(self) -> Option<PickerTarget> {
        match self.difficulty() {
            Some((arm, difficulty)) => Some(PickerTarget::BenchDifficulty { arm, difficulty }),
            None if matches!(self, Self::Classifier) => Some(PickerTarget::BenchClassifier),
            None => None,
        }
    }

    /// The row the model selector was opened on for `target`; `None` when
    /// it picks the session's model.
    #[must_use]
    pub const fn of_picker(target: PickerTarget) -> Option<Self> {
        match target {
            PickerTarget::Session => None,
            PickerTarget::BenchDifficulty { arm, difficulty } => {
                Some(Self::of_difficulty(arm, difficulty))
            }
            PickerTarget::BenchClassifier => Some(Self::Classifier),
        }
    }

    /// The arm (0 for A, 1 for B) and difficulty (0 easy, 1 medium, 2
    /// hard) of a row naming the model of one difficulty.
    #[must_use]
    pub const fn difficulty(self) -> Option<(usize, usize)> {
        match self {
            Self::AEasy => Some((0, 0)),
            Self::AMedium => Some((0, 1)),
            Self::AHard => Some((0, 2)),
            Self::BEasy => Some((1, 0)),
            Self::BMedium => Some((1, 1)),
            Self::BHard => Some((1, 2)),
            _ => None,
        }
    }

    /// The row naming the model of `difficulty` (0 easy, 1 medium, 2 hard)
    /// of `arm`.
    pub(super) const fn of_difficulty(arm: usize, difficulty: usize) -> Self {
        match (arm, difficulty) {
            (0, 0) => Self::AEasy,
            (0, 1) => Self::AMedium,
            (0, _) => Self::AHard,
            (_, 0) => Self::BEasy,
            (_, 1) => Self::BMedium,
            _ => Self::BHard,
        }
    }

    pub(super) fn index(self) -> usize {
        Self::ALL.iter().position(|f| *f == self).unwrap_or(0)
    }
}
