//! An integration's id: `[a-z0-9-]{1,32}`, starting with a letter.

use crate::error::IntegrationError;
use serde::{Deserialize, Serialize};

/// Longest id.
const MAX_LEN: usize = 32;

/// The stable id of an integration (`github`, `boxd`, `runpod`). It
/// names its stored secret, its settings and, with `-` made `_`, the
/// prefix of its agent tools.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct IntegrationId(String);

impl IntegrationId {
    /// `id`, checked.
    ///
    /// # Errors
    /// [`IntegrationError::InvalidId`] when it is empty, longer than 32,
    /// doesn't start with a letter, or has anything but `a-z`, `0-9`
    /// and `-`.
    pub fn new(id: impl Into<String>) -> Result<Self, IntegrationError> {
        let id = id.into();
        let valid = !id.is_empty()
            && id.len() <= MAX_LEN
            && id.starts_with(|c: char| c.is_ascii_lowercase())
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if valid {
            Ok(Self(id))
        } else {
            Err(IntegrationError::InvalidId(id))
        }
    }

    /// A built-in integration's fixed id, known to be valid (checked by
    /// the built-ins' tests).
    pub(crate) fn builtin(id: &'static str) -> Self {
        debug_assert!(Self::new(id).is_ok(), "{id} is not a valid id");
        Self(id.to_string())
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The prefix of its tools' names: the id with `-` made `_`.
    #[must_use]
    pub fn tool_prefix(&self) -> String {
        self.0.replace('-', "_")
    }
}

impl TryFrom<String> for IntegrationId {
    type Error = IntegrationError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::new(id)
    }
}

impl From<IntegrationId> for String {
    fn from(id: IntegrationId) -> Self {
        id.0
    }
}

impl std::fmt::Display for IntegrationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_short_lower_case_words() {
        for good in ["github", "boxd", "run-pod2", "a"] {
            assert!(IntegrationId::new(good).is_ok(), "{good}");
        }
        for bad in ["", "GitHub", "2fa", "-x", "a_b", "a b", &"x".repeat(33)] {
            assert!(IntegrationId::new(bad).is_err(), "{bad}");
        }
        assert_eq!(
            IntegrationId::new("run-pod").unwrap().tool_prefix(),
            "run_pod"
        );
    }
}
