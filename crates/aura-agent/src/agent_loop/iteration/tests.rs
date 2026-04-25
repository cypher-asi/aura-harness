//! Unit tests for the iteration submodule, split by behaviour cluster:
//!
//! - [`rate_limit_tests`] covers [`super::LlmCallError::from_reasoner_error`]
//!   and the looser prose-based rate-limit recovery path.
//! - [`max_tokens_tests`] covers [`super::handle_max_tokens`] and the
//!   `restore_budget_next_iteration` ↔ `LoopState::begin_iteration` contract.
//! - [`narration_budget_tests`] covers [`super::update_narration_budget`]
//!   soft / hard budget transitions.

mod max_tokens_tests;
mod narration_budget_tests;
mod rate_limit_tests;
