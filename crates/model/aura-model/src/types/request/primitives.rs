use crate::error::ReasonerError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::num::NonZeroU32;
use std::sync::Arc;

/// Model identifier — never empty.
///
/// Wraps `Arc<str>` so cloning is cheap (the agent loop clones the model name
/// into every request). Construct via `ModelName::try_new` (validating),
/// `ModelName::from("…")` (panics on empty — intended for call sites that have
/// already validated the input), or the explicit `From<String>` impl.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ModelName(Arc<str>);

impl Serialize for ModelName {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ModelName {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Self(Arc::from(s)))
    }
}

impl ModelName {
    /// Construct a new `ModelName`, rejecting empty / whitespace-only input.
    ///
    /// # Errors
    /// Returns [`ReasonerError::Internal`] if the model name is empty.
    pub fn try_new(name: impl Into<String>) -> Result<Self, ReasonerError> {
        let s = name.into();
        if s.trim().is_empty() {
            return Err(ReasonerError::Internal(
                "model name must not be empty".into(),
            ));
        }
        Ok(Self(Arc::from(s)))
    }

    /// Borrow the underlying string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ModelName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ModelName {
    /// Infallible conversion used by the builder. The builder re-validates on
    /// `build()`; callers that construct a `ModelName` directly from a `&str`
    /// must ensure the input is non-empty (kernel/agent code always does).
    fn from(s: &str) -> Self {
        Self(Arc::from(s))
    }
}

impl From<String> for ModelName {
    fn from(s: String) -> Self {
        Self(Arc::from(s))
    }
}

impl PartialEq<str> for ModelName {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for ModelName {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

/// Maximum output tokens — always > 0.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MaxTokens(NonZeroU32);

impl MaxTokens {
    /// Construct a validated `MaxTokens` value.
    ///
    /// # Errors
    /// Returns [`ReasonerError::Internal`] when `value == 0`.
    pub fn try_new(value: u32) -> Result<Self, ReasonerError> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or_else(|| ReasonerError::Internal("max_tokens must be > 0".into()))
    }

    /// Return the raw `u32` for downstream API serialization.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

impl From<MaxTokens> for u32 {
    fn from(v: MaxTokens) -> Self {
        v.get()
    }
}

/// Sampling temperature, constrained to the range supported by the major
/// model providers (`0.0..=2.0`). Default: `1.0`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct Temperature(f32);

impl Temperature {
    /// Construct a validated temperature.
    ///
    /// # Errors
    /// Returns [`ReasonerError::Internal`] when `value` is outside the
    /// allowed range (`0.0..=2.0`) or non-finite.
    pub fn try_new(value: f32) -> Result<Self, ReasonerError> {
        if value.is_finite() && (0.0..=2.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(ReasonerError::Internal(format!(
                "temperature {value} is outside the allowed range 0.0..=2.0"
            )))
        }
    }
}

impl Default for Temperature {
    fn default() -> Self {
        Self(1.0)
    }
}

impl From<Temperature> for f32 {
    fn from(v: Temperature) -> Self {
        v.0
    }
}

impl TryFrom<f32> for Temperature {
    type Error = ReasonerError;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

/// Wire value for OpenAI's `prompt_cache_retention` field. `InMemory`
/// maps to OpenAI's default 5-10 minute cache; `Hours24` requests
/// the extended 24-hour retention available on newer OpenAI models.
/// The router translates this into the OpenAI-native field when
/// forwarding; aura-harness only carries the hint on the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCacheRetention {
    InMemory,
    Hours24,
}

impl PromptCacheRetention {
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::InMemory => "in_memory",
            Self::Hours24 => "24h",
        }
    }
}
