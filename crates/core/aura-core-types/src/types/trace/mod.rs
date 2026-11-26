//! Run trajectory schema.
//!
//! A coding run is recorded as an ordered list of [`Step`]s. Each step
//! names who decided ([`Actor`]), what they saw ([`Observation`]), what
//! they chose ([`StepAction`]) and which policy chose it
//! ([`PolicyRef`]). [`Reward`]s are stored separately and point back at
//! a step or the whole run, so labels can arrive after the run without
//! rewriting it.
//!
//! The same shapes feed run scorecards, the terminal graph view, eval
//! comparisons and training-dataset exports. Bump
//! [`TRACE_SCHEMA_VERSION`] on any change that old readers would
//! misinterpret.
//!
//! Large observation payloads (system prompt, tool schemas, message
//! history, thinking) are stored out of line as content-addressed
//! blobs and referenced by [`BlobId`].

mod decision;
mod model;
mod reward;
mod step;
#[cfg(test)]
mod tests;

pub use decision::{DecisionSource, HarnessAction, HarnessDecision};
pub use model::{
    ModelAction, ModelExchange, ModelObservation, TokenUsage, ToolCallAction, ToolIntent,
};
pub use reward::{Reward, RewardKind, RewardTarget};
pub use step::{Actor, FileTouch, Observation, PolicyKind, PolicyRef, Step, StepAction, StepLinks};

/// Version of the on-disk trajectory format.
pub const TRACE_SCHEMA_VERSION: u32 = 1;

/// Hex-encoded BLAKE3 digest of a blob's bytes.
pub type BlobId = String;

/// Content address for `bytes`.
#[must_use]
pub fn blob_id(bytes: &[u8]) -> BlobId {
    blake3::hash(bytes).to_hex().to_string()
}
