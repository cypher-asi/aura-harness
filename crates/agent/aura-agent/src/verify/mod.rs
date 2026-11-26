//! Build and test verification helpers.
//!
//! This module provides standalone functions for:
//! - Running build/test commands and capturing results ([`runner`])
//! - Extracting type/method references from compiler errors ([`error_types`])
//! - Build/test command inference ([`utils`])
//! - Running the full project test suite ([`test`])

pub mod error_types;
pub mod runner;
pub mod test;
pub mod utils;

pub use error_types::parse_error_references;
pub use runner::{run_build_command, run_build_command_with_timeout};
pub use test::{run_full_test_suite, TestSuiteOutcome};
pub use utils::{infer_default_build_command, infer_default_test_command};
