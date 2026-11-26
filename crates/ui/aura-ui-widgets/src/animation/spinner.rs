//! Spinner animation for loading states.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Spinner frame sets.
const DOTS_FRAMES: &[&str] = &["◐", "◓", "◑", "◒"];
const BRAILLE_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const WAVE_FRAMES: &[&str] = &[
    "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█", "▇", "▆", "▅", "▄", "▃", "▂",
];
const BOX_FRAMES: &[&str] = &["┤", "┘", "┴", "└", "├", "┌", "┬", "┐"];

/// Spinner animation style.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpinnerStyle {
    /// Circle quarters: ◐ ◓ ◑ ◒
    #[default]
    Dots,
    /// Braille dots pattern
    Braille,
    /// Wave animation
    Wave,
    /// Box rotation
    Box,
}

impl SpinnerStyle {
    /// Get the frames for this style.
    #[must_use]
    const fn frames(self) -> &'static [&'static str] {
        match self {
            Self::Dots => DOTS_FRAMES,
            Self::Braille => BRAILLE_FRAMES,
            Self::Wave => WAVE_FRAMES,
            Self::Box => BOX_FRAMES,
        }
    }
}

/// Animated spinner for loading states.
#[derive(Debug, Clone)]
pub struct Spinner {
    /// Animation style
    style: SpinnerStyle,
    /// Current frame index
    frame_index: usize,
    /// Last update time
    last_update: Instant,
    /// Frame interval
    interval: Duration,
}

impl Spinner {
    /// Create a new spinner with default style.
    #[must_use]
    pub fn new() -> Self {
        Self {
            style: SpinnerStyle::default(),
            frame_index: 0,
            last_update: Instant::now(),
            interval: Duration::from_millis(80),
        }
    }

    /// Create a spinner with a specific style.
    #[must_use]
    pub fn with_style(style: SpinnerStyle) -> Self {
        Self {
            style,
            frame_index: 0,
            last_update: Instant::now(),
            interval: Duration::from_millis(80),
        }
    }

    /// Set the animation interval.
    #[must_use]
    pub const fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Get the current frame and advance if needed.
    #[must_use]
    pub fn tick(&mut self) -> &'static str {
        let frames = self.style.frames();
        if self.last_update.elapsed() >= self.interval {
            self.frame_index = (self.frame_index + 1) % frames.len();
            self.last_update = Instant::now();
        }
        frames[self.frame_index]
    }

    /// Get the current frame without advancing.
    #[must_use]
    pub fn current(&self) -> &'static str {
        let frames = self.style.frames();
        frames[self.frame_index]
    }

    /// Reset the spinner to the first frame.
    pub fn reset(&mut self) {
        self.frame_index = 0;
        self.last_update = Instant::now();
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spinner_creation() {
        let spinner = Spinner::new();
        assert_eq!(spinner.current(), "◐");
    }

    #[test]
    fn test_spinner_styles() {
        let styles = [
            SpinnerStyle::Dots,
            SpinnerStyle::Braille,
            SpinnerStyle::Wave,
            SpinnerStyle::Box,
        ];

        for style in styles {
            let mut spinner = Spinner::with_style(style);
            let frame = spinner.tick();
            assert!(!frame.is_empty());
        }
    }

    #[test]
    fn test_spinner_reset() {
        let mut spinner = Spinner::new();
        spinner.frame_index = 3;
        spinner.reset();
        assert_eq!(spinner.frame_index, 0);
    }
}
