//! What a command has printed so far, while it is still running.

use std::time::{Duration, Instant};

/// Bytes of a running command's output kept; older output is dropped.
const KEEP_BYTES: usize = 64 * 1024;

/// How long a running command can print nothing before it is called quiet.
pub const QUIET_AFTER: Duration = Duration::from_secs(30);

/// The end of a running command's output, and when it last printed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveOutput {
    text: String,
    last_at: Option<Instant>,
}

impl LiveOutput {
    /// Add `chunk`, dropping the oldest output beyond [`KEEP_BYTES`].
    pub fn push(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        self.text.push_str(chunk);
        self.last_at = Some(Instant::now());
        if self.text.len() > KEEP_BYTES {
            let mut cut = self.text.len() - KEEP_BYTES;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            self.text.drain(..cut);
        }
    }

    /// Forget the output, once the command's result replaces it.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Whether nothing was printed yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// When the command last printed, if it did.
    #[must_use]
    pub const fn last_at(&self) -> Option<Instant> {
        self.last_at
    }

    /// The last `max` non-blank lines as a terminal would show them: a
    /// line redrawn with `\r` (progress bars) keeps only its final text,
    /// and ANSI styling is removed.
    #[must_use]
    pub fn tail(&self, max: usize) -> Vec<String> {
        last_lines(&self.text, max)
            .iter()
            .map(|line| crate::ansi::strip_ansi(line))
            .collect()
    }

    /// [`Self::tail`] with the ANSI styling kept, for a terminal view.
    #[must_use]
    pub fn styled_tail(&self, max: usize) -> Vec<String> {
        last_lines(&self.text, max)
    }
}

fn last_lines(text: &str, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = text
        .split('\n')
        .rev()
        .map(|line| {
            let line = line.trim_end_matches('\r');
            line.rsplit('\r')
                .next()
                .unwrap_or(line)
                .trim_end()
                .to_string()
        })
        .filter(|line| !crate::ansi::strip_ansi(line).trim().is_empty())
        .take(max)
        .collect();
    lines.reverse();
    lines
}

/// How long a running command that started at `started` has printed
/// nothing, once that reaches [`QUIET_AFTER`].
#[must_use]
pub fn quiet_for(started: Option<Instant>, live: &LiveOutput) -> Option<Duration> {
    live.last_at()
        .or(started)
        .map(|since| since.elapsed())
        .filter(|quiet| *quiet >= QUIET_AFTER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_last_lines_as_a_terminal_shows_them() {
        let mut live = LiveOutput::default();
        live.push("one\n\x1b[32mtwo\x1b[0m\n\n");
        live.push("10%\r50%\r100%\r\nthree\n");
        assert_eq!(live.tail(3), ["two", "100%", "three"]);
        assert_eq!(live.tail(10), ["one", "two", "100%", "three"]);
    }

    #[test]
    fn a_partial_last_line_is_shown() {
        let mut live = LiveOutput::default();
        live.push("Compiling a\nCompiling b");
        assert_eq!(live.tail(1), ["Compiling b"]);
    }

    #[test]
    fn old_output_is_dropped_on_a_char_boundary() {
        let mut live = LiveOutput::default();
        live.push(&"é".repeat(KEEP_BYTES));
        live.push("\nend");
        assert!(live.text.len() <= KEEP_BYTES);
        assert_eq!(live.tail(1), ["end"]);
    }

    #[test]
    fn quiet_counts_from_the_last_output_or_the_start() {
        let long_ago = Instant::now().checked_sub(QUIET_AFTER * 2);
        let mut live = LiveOutput::default();
        assert!(quiet_for(long_ago, &live).is_some(), "never printed");
        live.push("x");
        assert!(quiet_for(long_ago, &live).is_none(), "just printed");
        assert!(quiet_for(Some(Instant::now()), &LiveOutput::default()).is_none());
    }
}
