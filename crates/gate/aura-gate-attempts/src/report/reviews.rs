//! The reviews' table: how the reviews held up, per way of asking and
//! reviewer.

use std::fmt::{self, Write as _};

use crate::score::Report;

/// Width of the column of how the reviewer was asked.
const ASKED_WIDTH: usize = 25;

/// Width of the reviewer's column.
const REVIEWER_WIDTH: usize = 20;

impl Report {
    /// How the reviews held up; nothing when there were none.
    pub(super) fn write_reviews(&self, out: &mut String) -> fmt::Result {
        if self.reviews.is_empty() {
            return Ok(());
        }
        writeln!(out, "\nReviews of work that was about to stand:")?;
        writeln!(
            out,
            "  {:ASKED_WIDTH$}  {:REVIEWER_WIDTH$}  reviewed  known  wrong  said unfinished  caught  right but flagged  wrong but passed     cost",
            "asked", "reviewer"
        )?;
        for score in &self.reviews {
            let replayed = if score.replayed { " (replayed)" } else { "" };
            let asked = format!("{}{replayed}", score.variant);
            let reviewer = score.judge.rsplit('/').next().unwrap_or_default();
            let cost = cost_cell(score.cost_usd, score.unpriced);
            writeln!(
                out,
                "  {asked:ASKED_WIDTH$}  {reviewer:REVIEWER_WIDTH$}  {:>8}  {:>5}  {:>5}  {:>15}  {:>6}  {:>17}  {:>16}  {cost}",
                score.reviewed,
                score.known,
                score.wrong,
                score.unfinished,
                score.caught,
                score.false_alarms,
                score.wrong - score.caught,
            )?;
        }
        Ok(())
    }
}

/// `$1.25`; `$1.25+?` when `unpriced` reviews had no known price, and
/// `unknown` when none had one.
fn cost_cell(cost_usd: f64, unpriced: u32) -> String {
    match unpriced {
        0 => format!("${cost_usd:>6.2}"),
        _ if cost_usd > 0.0 => format!("${cost_usd:>6.2}+?"),
        _ => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::cost_cell;

    #[test]
    fn a_review_cost_says_when_some_prices_are_unknown() {
        assert_eq!(cost_cell(0.15, 0), "$  0.15");
        assert_eq!(cost_cell(0.15, 2), "$  0.15+?");
        assert_eq!(cost_cell(0.0, 2), "unknown");
        assert_eq!(cost_cell(0.0, 0), "$  0.00");
    }
}
