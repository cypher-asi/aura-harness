//! [`BootConsole::transcript`]: the console as plain text to copy.

use super::{boot_duration, wait_clock, BootConsole, BootStep};
use aura_ui::events::BootStageState;
use std::fmt::Write as _;

impl BootConsole {
    /// The console as plain text to copy: how it went, its steps, the
    /// facts and its whole log, the lines dropped from memory read back
    /// from the spill file.
    #[must_use]
    pub fn transcript(&self) -> String {
        let mut out = self.transcript_heading();
        out.push_str(&self.transcript_steps());
        out.push_str(&self.transcript_facts());
        out.push_str(&self.transcript_log());
        out
    }

    /// The title and how it went.
    fn transcript_heading(&self) -> String {
        let mut out = self.title.clone();
        match &self.outcome {
            None => out.push_str(" (still going)\n"),
            Some(outcome) => {
                let how = if outcome.ok { "done" } else { "failed" };
                let took = boot_duration(outcome.elapsed_ms);
                let _ = writeln!(out, " ({how} after {took})\n{}", outcome.summary);
            }
        }
        out
    }

    /// One line per step, after a blank line when there are any.
    fn transcript_steps(&self) -> String {
        let mut out = String::new();
        if !self.steps.is_empty() {
            out.push('\n');
        }
        for step in &self.steps {
            transcript_step(&mut out, step);
        }
        out
    }

    /// The facts, keys aligned, after a blank line when there are any.
    fn transcript_facts(&self) -> String {
        let mut out = String::new();
        if !self.facts.is_empty() {
            out.push('\n');
        }
        let key_width = self.facts.iter().map(|(k, _)| k.chars().count()).max();
        for (key, value) in &self.facts {
            let _ = writeln!(
                out,
                "{key:<width$}  {value}",
                width = key_width.unwrap_or(0)
            );
        }
        out
    }

    /// The whole log, the spilled lines first, after a blank line when
    /// there is any.
    fn transcript_log(&self) -> String {
        let mut out = String::new();
        if self.dropped + self.log.len() > 0 {
            out.push('\n');
        }
        let spilled = self.spill.as_ref().and_then(|spill| {
            let mut spill = spill.lock();
            let count = spill.starts.len();
            spill.read(0, count).ok()
        });
        match spilled {
            Some(lines) => {
                for line in lines {
                    out.push_str(&line.text);
                    out.push('\n');
                }
            }
            None if self.dropped > 0 => {
                let _ = writeln!(out, "… {} earlier lines could not be kept", self.dropped);
            }
            None => {}
        }
        for line in &self.log {
            out.push_str(&line.text);
            out.push('\n');
        }
        out
    }
}

/// Write `step`'s transcript line to `out`.
fn transcript_step(out: &mut String, step: &BootStep) {
    let label = &step.label;
    let _ = match &step.state {
        BootStageState::Pending => writeln!(out, "· {label}"),
        BootStageState::Running => match &step.waiting {
            Some(wait) => writeln!(
                out,
                "… {label} (running; waiting on {}, {})",
                wait.what,
                wait_clock(wait)
            ),
            None => writeln!(out, "… {label} (running)"),
        },
        BootStageState::Done { ms } => writeln!(out, "✓ {label}  {}", boot_duration(*ms)),
        BootStageState::Skipped(why) => writeln!(out, "– {label}  skipped: {why}"),
        BootStageState::Failed(why) => writeln!(out, "✗ {label}  {why}"),
    };
}
