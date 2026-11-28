//! The status dot: how a thing stands, as one glyph in one color, the
//! only colored cell of its line. Every panel that marks an entry's state
//! with a dot (a channel's QUEUE, the Integrations panel) takes it from
//! here, so a state looks the same everywhere. The text beside it keeps
//! its own plain color.
//!
//! A filled `●` means the thing is there, and its color says how it is
//! doing. A hollow `○` means it is not there or not set up.

use crate::themes::Theme;
use ratatui::{
    style::{Color, Style},
    text::Span,
};

/// How the thing a dot marks stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusDot {
    /// Done, up or connected: `●` in `success`
    Ok,
    /// Under way: `●` in `warning`
    Busy,
    /// There, waiting for its turn: `●` in `muted`
    Waiting,
    /// Failed: `●` in `error`
    Failed,
    /// Not there or not set up: `○` in `muted`
    Off,
}

impl StatusDot {
    /// The dot's glyph: hollow for [`Self::Off`], else filled.
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Off => "○",
            _ => "●",
        }
    }

    /// The dot's color in `theme`.
    #[must_use]
    pub const fn color(self, theme: &Theme) -> Color {
        let colors = &theme.colors;
        match self {
            Self::Ok => colors.success,
            Self::Busy => colors.warning,
            Self::Waiting | Self::Off => colors.muted,
            Self::Failed => colors.error,
        }
    }

    /// The dot as a span: its glyph in its color, nothing else styled.
    #[must_use]
    pub fn span(self, theme: &Theme) -> Span<'static> {
        Span::styled(self.glyph(), Style::default().fg(self.color(theme)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    #[test]
    fn each_state_has_its_own_look_and_no_weight() {
        let theme = Theme::default();
        let all = [
            StatusDot::Ok,
            StatusDot::Busy,
            StatusDot::Waiting,
            StatusDot::Failed,
            StatusDot::Off,
        ];
        for (at, dot) in all.iter().enumerate() {
            let span = dot.span(&theme);
            assert_eq!(span.content, dot.glyph());
            assert_eq!(span.style.fg, Some(dot.color(&theme)));
            assert!(!span.style.add_modifier.contains(Modifier::BOLD));
            for other in &all[at + 1..] {
                let looks = |d: &StatusDot| (d.glyph(), d.color(&theme));
                assert_ne!(looks(dot), looks(other), "{dot:?} {other:?}");
            }
        }
    }

    #[test]
    fn only_off_is_hollow() {
        assert_eq!(StatusDot::Off.glyph(), "○");
        for dot in [
            StatusDot::Ok,
            StatusDot::Busy,
            StatusDot::Waiting,
            StatusDot::Failed,
        ] {
            assert_eq!(dot.glyph(), "●", "{dot:?}");
        }
    }
}
