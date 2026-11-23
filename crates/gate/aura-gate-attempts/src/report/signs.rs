//! The signs' tables: how each sign did on the attempts Gate let stand,
//! and what each has earned.

use std::fmt::{self, Write as _};

use super::share;
use crate::score::Report;
use crate::tiers::{ACT_MIN_RAISED, REVIEW_LIFT, REVIEW_MIN_RAISED};

/// Width of the sign column: the length of the longest sign's name.
const SIGN_WIDTH: usize = 26;

impl Report {
    /// How each sign did on the attempts Gate let stand.
    pub(super) fn write_flags(&self, out: &mut String) -> fmt::Result {
        let right = self.stood - self.stood_wrong;
        writeln!(
            out,
            "\nSigns of failure, scored on the {} attempts Gate let stand ({} wrong, {right} right):",
            self.stood, self.stood_wrong
        )?;
        writeln!(
            out,
            "  {:SIGN_WIDTH$}  raised  caught  of wrong  false alarms  of right  precision",
            "sign"
        )?;
        for score in &self.flags {
            writeln!(
                out,
                "  {:SIGN_WIDTH$}  {:>6}  {:>6}  {:>8}  {:>12}  {:>8}  {:>9}",
                score.flag.name(),
                score.raised,
                score.caught,
                share(score.caught, self.stood_wrong),
                score.false_alarms,
                share(score.false_alarms, right),
                share(score.caught, score.raised),
            )?;
        }
        Ok(())
    }

    /// What each sign has earned, what it lacks for the next tier, and
    /// what earns each tier.
    pub(super) fn write_tiers(&self, out: &mut String) -> fmt::Result {
        let tiers = &self.tiers;
        writeln!(
            out,
            "\nWhat each sign has earned, on the {} routed attempts below the strongest tier whose work is known ({} wrong):",
            tiers.known, tiers.wrong
        )?;
        writeln!(
            out,
            "  {:SIGN_WIDTH$}  raised  wrong  by chance  tier      next",
            "sign"
        )?;
        for sign in &tiers.signs {
            writeln!(
                out,
                "  {:SIGN_WIDTH$}  {:>6}  {:>5}  {:>9.1}  {:8}  {}",
                sign.flag.name(),
                sign.raised,
                sign.wrong,
                sign.expected,
                sign.tier.name(),
                sign.lacks()
            )?;
        }
        writeln!(
            out,
            "  review: raised {REVIEW_MIN_RAISED} times or more, and wrong {REVIEW_LIFT} times as often as attempts of the same difficulty; acts: raised {ACT_MIN_RAISED} times or more, and wrong 4 times in 5"
        )
    }
}
