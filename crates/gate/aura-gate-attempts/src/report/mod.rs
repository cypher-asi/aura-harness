//! The report as text, for a terminal: how Gate's verdicts held up,
//! overall and per model, then the signs' tables, the reviews, and the
//! failed work Gate let stand.

mod reviews;
mod signs;

use std::fmt::{self, Write as _};

use crate::score::Report;

/// What a report says when no attempt's work is known right or wrong.
const NOTHING_TO_SCORE: &str = "Nothing to score yet: run /compare on benchmark tasks, whose own checks say whether each run's work was right.\n";

/// The narrowest the model column is: as wide as its heading.
const MODEL_WIDTH: usize = 5;

impl Report {
    /// The report as text, for a terminal.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        // Writing to a `String` cannot fail.
        let _ = self.write_to(&mut out);
        out
    }

    fn write_to(&self, out: &mut String) -> fmt::Result {
        self.write_headline(out)?;
        if self.verdicts.known == 0 {
            return out.write_str(NOTHING_TO_SCORE);
        }
        self.write_stood(out)?;
        self.write_models(out)?;
        self.write_flags(out)?;
        self.write_tiers(out)?;
        self.write_reviews(out)?;
        self.write_missed(out)
    }

    /// How many attempts are logged, and how many were shown right or
    /// wrong.
    fn write_headline(&self, out: &mut String) -> fmt::Result {
        let v = self.verdicts;
        writeln!(
            out,
            "{} attempts logged ({} routed by Gate), {} later shown right or wrong: {} right, {} wrong",
            self.attempts,
            self.routed,
            v.known,
            v.right,
            v.known - v.right
        )
    }

    /// What Gate let stand, and what it counted as failed that was right.
    fn write_stood(&self, out: &mut String) -> fmt::Result {
        let v = self.verdicts;
        writeln!(
            out,
            "Gate let stand {} of them: {} right, {} wrong ({} it counted as passed, {} it had nothing to judge by)",
            self.stood, self.stood - self.stood_wrong, self.stood_wrong, v.false_passes, v.unjudged_failures
        )?;
        writeln!(
            out,
            "Gate counted {} as failed whose work was right",
            v.false_failures
        )
    }

    /// The attempts Gate let stand per model, and how many were wrong.
    fn write_models(&self, out: &mut String) -> fmt::Result {
        writeln!(out, "\nPer model, attempts Gate let stand:")?;
        let width = self
            .models
            .keys()
            .map(String::len)
            .fold(MODEL_WIDTH, usize::max);
        writeln!(out, "  {:width$}  stood  wrong  share wrong", "model")?;
        for (model, m) in &self.models {
            let wrong = m.known - m.right;
            writeln!(
                out,
                "  {model:width$}  {:>5}  {:>5}  {:>11}",
                m.known,
                wrong,
                share(wrong, m.known)
            )?;
        }
        Ok(())
    }

    /// The failed work Gate let stand, each with the signs it raised.
    fn write_missed(&self, out: &mut String) -> fmt::Result {
        if self.stood_wrong == 0 {
            return Ok(());
        }
        writeln!(out, "\nFailed work Gate let stand:")?;
        for missed in self.missed() {
            let names = missed
                .flags
                .iter()
                .map(|flag| flag.name())
                .collect::<Vec<_>>()
                .join(", ");
            let flags = if names.is_empty() {
                "no sign raised"
            } else {
                names.as_str()
            };
            let what = [missed.run.as_str(), missed.task.as_str()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            writeln!(
                out,
                "  {what} · {} · gate: {} · {flags}",
                missed.model, missed.signal
            )?;
        }
        Ok(())
    }
}

/// `part` as a whole percentage of `of`, or `-` when `of` is none.
fn share(part: u32, of: u32) -> String {
    if of == 0 {
        "-".to_string()
    } else {
        format!("{:.0}%", 100.0 * f64::from(part) / f64::from(of))
    }
}

#[cfg(test)]
mod tests;
