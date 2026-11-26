//! Bookkeeping kept from tick to tick: the animation frame, whether the
//! screen needs drawing again, and the alerts waiting for the bell.

use crate::alerts::AlertSettings;
use std::time::Instant;

/// Animation, redraw and alert state carried between ticks.
#[derive(Debug)]
pub(crate) struct Ticks {
    /// Animation frame counter for spinners
    pub(crate) animation_frame: usize,
    /// Ticks until spinners turn again while nothing else animates
    pub(crate) spinner_wait: usize,
    /// Whether the screen may look different since the host last drew
    /// it; see [`super::App::take_redraw`]
    pub(crate) redraw: bool,
    /// Whether a tool may have changed files since the tree was read; the
    /// next tick reads it again, once however many tools finished
    pub(crate) sandbox_stale: bool,
    /// When the in-flight turn started (`None` unless processing)
    pub(crate) processing_since: Option<Instant>,
    /// Messages for the user that the terminal should ring the bell for
    pub(crate) alerts: Vec<String>,
    /// When to get the user's attention
    pub(crate) alert_settings: AlertSettings,
    /// Whether verbose mode is enabled
    pub(crate) verbose: bool,
}

impl Default for Ticks {
    /// Nothing animating or pending, but the first frame still to draw.
    fn default() -> Self {
        Self {
            animation_frame: 0,
            spinner_wait: 0,
            redraw: true,
            sandbox_stale: false,
            processing_since: None,
            alerts: Vec::new(),
            alert_settings: AlertSettings::default(),
            verbose: false,
        }
    }
}
