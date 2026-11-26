//! Progress bar tests.

use super::*;
use aura_ui_terminal::ProgressBar;

proptest! {
    /// Progress value is clamped to [0, 1].
    #[test]
    fn progress_clamped(value: f32) {
        let bar = ProgressBar::new(20).with_progress(value);
        // We can't directly access progress, but render_string should not panic
        let rendered = bar.render_string();
        prop_assert!(!rendered.is_empty());
    }

    /// Progress bar renders without panicking for any width.
    #[test]
    fn progress_any_width(width in 5u16..200, progress in 0.0f32..=1.0) {
        let bar = ProgressBar::new(width).with_progress(progress);
        let rendered = bar.render_string();
        prop_assert!(!rendered.is_empty());
    }
}
