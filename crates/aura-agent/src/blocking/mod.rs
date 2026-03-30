//! Blocking detection for the agent loop.
//!
//! Prevents infinite loops by detecting and blocking repeated tool calls
//! that are not making progress. Implements 7 detectors:
//!
//! 1. Duplicate writes to the same path
//! 2. Write failures exceeding threshold
//! 3. Consecutive command failures
//! 4. Exploration allowance exceeded
//! 5. Read guard limits
//! 6. Write cooldowns
//! 7. Shell read workarounds

pub(crate) mod detection;
pub(crate) mod stall;
