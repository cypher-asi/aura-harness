//! Unit tests for the iteration submodule, split by behaviour cluster:
//!
//! - [`rate_limit_tests`] covers [`super::LlmCallError::from_reasoner_error`]
//!   and the looser prose-based rate-limit recovery path.
mod rate_limit_tests;
