//! Unit tests for the iteration submodule, split by behaviour cluster:
//!
//! - [`rate_limit_tests`] covers [`super::LlmCallError::from_reasoner_error`]
//!   and the looser prose-based rate-limit recovery path.
//! - [`append_paragraph_tests`] covers how text blocks are joined into
//!   the turn's `total_text`.
mod append_paragraph_tests;
mod rate_limit_tests;
