//! Stall detection — detects when the agent is stuck repeating the same operations.

use crate::constants::STALL_STREAK_THRESHOLD;
use std::collections::HashSet;

/// Tracks write targets across iterations to detect stalls.
#[derive(Debug, Default)]
pub struct StallDetector {
    /// Previous iteration's write targets.
    prev_targets: HashSet<String>,
    /// Streak of identical write targets.
    streak: usize,
}

impl StallDetector {
    /// Update the detector with this iteration's write targets.
    ///
    /// Returns `true` if a stall is detected (same targets for
    /// `STALL_STREAK_THRESHOLD` iterations).
    pub fn update(&mut self, current_targets: &HashSet<String>, any_success: bool) -> bool {
        if current_targets.is_empty() || any_success {
            self.streak = 0;
            self.prev_targets.clone_from(current_targets);
            return false;
        }

        if *current_targets == self.prev_targets {
            self.streak += 1;
        } else {
            self.streak = 1;
            self.prev_targets.clone_from(current_targets);
        }

        self.streak >= STALL_STREAK_THRESHOLD
    }

    /// Get the current streak count.
    #[must_use]
    pub const fn streak(&self) -> usize {
        self.streak
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_writes_resets() {
        let mut det = StallDetector::default();
        let empty = HashSet::new();
        assert!(!det.update(&empty, false));
        assert_eq!(det.streak(), 0);
    }

    #[test]
    fn test_successful_edit_resets() {
        let mut det = StallDetector::default();
        let targets: HashSet<String> = ["a.rs".to_string()].into();
        assert!(!det.update(&targets, true));
        assert_eq!(det.streak(), 0);
    }

    #[test]
    fn test_identical_content_increments() {
        let mut det = StallDetector::default();
        let targets: HashSet<String> = ["a.rs".to_string()].into();
        det.update(&targets, false);
        assert_eq!(det.streak(), 1);
        det.update(&targets, false);
        assert_eq!(det.streak(), 2);
    }

    #[test]
    fn test_triggers_at_streak_3() {
        let mut det = StallDetector::default();
        let targets: HashSet<String> = ["a.rs".to_string()].into();
        assert!(!det.update(&targets, false)); // 1
        assert!(!det.update(&targets, false)); // 2
        assert!(det.update(&targets, false)); // 3 = threshold
    }

    #[test]
    fn test_different_targets_reset_streak() {
        let mut det = StallDetector::default();
        let targets_a: HashSet<String> = ["a.rs".to_string()].into();
        let targets_b: HashSet<String> = ["b.rs".to_string()].into();
        assert!(!det.update(&targets_a, false)); // 1
        assert!(!det.update(&targets_a, false)); // 2
        assert!(!det.update(&targets_b, false)); // reset to 1
        assert!(!det.update(&targets_b, false)); // 2
        assert!(det.update(&targets_b, false)); // 3 = threshold
    }

    #[test]
    fn test_success_resets_mid_streak() {
        let mut det = StallDetector::default();
        let targets: HashSet<String> = ["a.rs".to_string()].into();
        assert!(!det.update(&targets, false)); // 1
        assert!(!det.update(&targets, false)); // 2
        assert!(!det.update(&targets, true)); // success resets
        assert_eq!(det.streak(), 0);
        assert!(!det.update(&targets, false)); // 1 again
        assert!(!det.update(&targets, false)); // 2
        assert!(det.update(&targets, false)); // 3 = threshold
    }

    #[test]
    fn test_multiple_targets_same_set() {
        let mut det = StallDetector::default();
        let targets: HashSet<String> = ["a.rs".to_string(), "b.rs".to_string()].into();
        assert!(!det.update(&targets, false));
        assert!(!det.update(&targets, false));
        assert!(det.update(&targets, false));
    }

    #[test]
    fn test_streak_accessor() {
        let det = StallDetector::default();
        assert_eq!(det.streak(), 0);
    }
}
