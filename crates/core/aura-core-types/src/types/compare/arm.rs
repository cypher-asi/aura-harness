use std::borrow::Cow;

use super::super::gate::Difficulty;
use serde::{Deserialize, Serialize};

/// Which side of a comparison a run is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CompareArm {
    /// The first arm; Gate in comparisons recorded before arms were
    /// chosen freely.
    #[serde(rename = "a", alias = "gate")]
    A,
    /// The second arm; the model Gate was compared with in older records.
    #[serde(rename = "b", alias = "other")]
    B,
}

impl CompareArm {
    /// Both arms, A first.
    pub const BOTH: [Self; 2] = [Self::A, Self::B];

    /// The arm's name in run names and commands: `a` or `b`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }

    /// The other arm.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }

    /// 0 for A, 1 for B.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => 1,
        }
    }
}

impl std::fmt::Display for CompareArm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What one arm runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ArmSpec {
    /// Gate routes every turn and subtask, with exploration off;
    /// `delegate` says whether its lead must delegate.
    Gate { delegate: bool },
    /// Every turn and subtask runs on `model`.
    Model { model: String },
    /// Every turn and subtask is classified and runs on the model given
    /// for its difficulty.
    ByDifficulty {
        /// The model easy work runs on.
        easy: String,
        /// The model medium work, and work of unknown difficulty, runs on.
        medium: String,
        /// The model hard work runs on.
        hard: String,
    },
}

impl ArmSpec {
    /// How [`ArmSpec::label`] names Gate whose lead need not delegate.
    pub const NO_DELEGATE: &'static str = "gate:nodelegate";

    /// What [`ArmSpec::label`] starts a by-difficulty arm with, as in
    /// `difficulty:easy=<id>,medium=<id>,hard=<id>`.
    pub const BY_DIFFICULTY_PREFIX: &'static str = "difficulty:";

    /// `gate`, [`Self::NO_DELEGATE`], the model, or
    /// `difficulty:easy=<id>,medium=<id>,hard=<id>`.
    #[must_use]
    pub fn label(&self) -> Cow<'_, str> {
        match self {
            Self::Gate { delegate: true } => Cow::Borrowed("gate"),
            Self::Gate { delegate: false } => Cow::Borrowed(Self::NO_DELEGATE),
            Self::Model { model } => Cow::Borrowed(model),
            Self::ByDifficulty { easy, medium, hard } => Cow::Owned(format!(
                "{}easy={easy},medium={medium},hard={hard}",
                Self::BY_DIFFICULTY_PREFIX
            )),
        }
    }

    /// The arm a label from [`Self::label`] names. A label that is
    /// neither Gate nor a well-formed by-difficulty label names a model.
    #[must_use]
    pub fn from_label(label: &str) -> Self {
        match label {
            "gate" => Self::Gate { delegate: true },
            Self::NO_DELEGATE => Self::Gate { delegate: false },
            model => match Self::parse_by_difficulty(model) {
                Some(Ok([easy, medium, hard])) => Self::ByDifficulty { easy, medium, hard },
                _ => Self::Model {
                    model: model.to_string(),
                },
            },
        }
    }

    /// The easy, medium and hard models of a by-difficulty label:
    /// `None` when `label` does not start with
    /// [`Self::BY_DIFFICULTY_PREFIX`] (in any case). The keys come in any
    /// order, as `key=value` separated by commas.
    ///
    /// # Errors
    /// Says which key is missing, given twice or unknown, or which entry
    /// is not `key=value`.
    pub fn parse_by_difficulty(label: &str) -> Option<Result<[String; 3], String>> {
        let prefix = Self::BY_DIFFICULTY_PREFIX;
        let head = label.trim_start();
        if !head
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
        {
            return None;
        }
        let body = head.get(prefix.len()..).unwrap_or_default();
        Some(parse_difficulty_models(body))
    }

    /// The model work of `difficulty` runs on: that of its difficulty
    /// (medium when unknown) for a by-difficulty arm, the model of a
    /// model arm, and `None` for Gate.
    #[must_use]
    pub fn model_for(&self, difficulty: Option<Difficulty>) -> Option<&str> {
        match self {
            Self::Gate { .. } => None,
            Self::Model { model } => Some(model),
            Self::ByDifficulty { easy, medium, hard } => Some(match difficulty {
                Some(Difficulty::Easy) => easy,
                Some(Difficulty::Hard) => hard,
                Some(Difficulty::Medium) | None => medium,
            }),
        }
    }

    /// Whether Gate routes the arm.
    #[must_use]
    pub const fn is_gate(&self) -> bool {
        matches!(self, Self::Gate { .. })
    }

    /// Whether the arm picks its model by the difficulty of the work.
    #[must_use]
    pub const fn is_by_difficulty(&self) -> bool {
        matches!(self, Self::ByDifficulty { .. })
    }
}

/// The easy, medium and hard models of `body`, the part of a
/// by-difficulty label after its prefix.
fn parse_difficulty_models(body: &str) -> Result<[String; 3], String> {
    let (mut easy, mut medium, mut hard) = (None, None, None);
    for entry in body.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let Some((key, value)) = entry.split_once('=') else {
            return Err(format!("`{entry}` is not difficulty=model"));
        };
        let key = key.trim().to_ascii_lowercase();
        let difficulty = Difficulty::from_name(&key)
            .ok_or_else(|| format!("Unknown difficulty `{key}`; give easy, medium and hard"))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("No model given for {difficulty}"));
        }
        let slot = match difficulty {
            Difficulty::Easy => &mut easy,
            Difficulty::Medium => &mut medium,
            Difficulty::Hard => &mut hard,
        };
        if slot.is_some() {
            return Err(format!("{difficulty} is given twice"));
        }
        *slot = Some(value.to_string());
    }
    let given = |model: Option<String>, difficulty: Difficulty| {
        model.ok_or_else(|| format!("No model given for {difficulty}"))
    };
    Ok([
        given(easy, Difficulty::Easy)?,
        given(medium, Difficulty::Medium)?,
        given(hard, Difficulty::Hard)?,
    ])
}
