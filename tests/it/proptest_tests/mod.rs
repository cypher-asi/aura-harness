//! Property-based tests using proptest.
//!
//! These tests verify invariants using randomly generated inputs.

// Integration-test crate: test code may unwrap/expect.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use proptest::prelude::*;

// Note: History tests are in aura-ui-terminal's internal test module
// since InputHistory is not re-exported publicly

mod hash_chain_tests;
mod hash_tests;
mod id_tests;
mod key_tests;
mod progress_tests;
mod sandbox_tests;
mod serialization_tests;
mod transaction_chain_tests;
