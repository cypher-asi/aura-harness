//! How Gate's verdicts held up: what it counted as passed or failed, or
//! had nothing to judge by, against what the work proved to be.

use serde::Serialize;

/// How Gate's verdicts on a set of attempts held up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Verdicts {
    /// Attempts whose work was later shown right or wrong
    pub known: u32,
    /// Of those, the ones whose work was right
    pub right: u32,
    /// Gate counted the attempt as passed, and its work was wrong
    pub false_passes: u32,
    /// Gate had nothing to judge the attempt by, and its work was wrong
    pub unjudged_failures: u32,
    /// Gate counted the attempt as failed, and its work was right
    pub false_failures: u32,
}

impl Verdicts {
    /// Count an attempt Gate said `said` of (passed, failed, or nothing)
    /// whose work was `right`.
    pub(super) fn take(&mut self, said: Option<bool>, right: bool) {
        self.known += 1;
        self.right += u32::from(right);
        match (said, right) {
            (Some(true), false) => self.false_passes += 1,
            (None, false) => self.unjudged_failures += 1,
            (Some(false), true) => self.false_failures += 1,
            _ => {}
        }
    }
}
