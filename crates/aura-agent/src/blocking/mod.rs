//! Blocking detection for the agent loop.
//!
//! Prevents infinite loops by detecting and blocking repeated tool calls
//! that are not making progress. Implements 6 detectors:
//!
//! 0. Missing required arguments
//! 1. Duplicate writes to the same path
//! 2. Write failures exceeding threshold
//! 3. Consecutive command failures
//! 4. Exploration allowance exceeded
//! 5. Read guard limits
//! 6. Write cooldowns

pub mod detection;
pub mod stall;
