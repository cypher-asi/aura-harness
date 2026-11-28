//! The status dot: `●` in the color of how a thing stands, the one
//! colored cell of its line. Every panel that marks an entry's state with
//! a dot (a channel's QUEUE, the integrations) takes it from here, so the
//! same state is the same color everywhere; the text beside it keeps its
//! own plain color.

use crate::themes::Theme;
use ratatui::{
    style::{Color, Style},
    text::Span,
};

/// How the thing a dot marks stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusDot {
    /// Done or up: `success`
    Ok,
    /// Under way: `warning`
    Busy,
    /// Waiting for its turn: `muted`
    Waiting,
    /// Failed: `error`
    Failed,
    /// Off or not set up: `faint`
    Off,
}

impl StatusDot {
    /// The glyph every dot is drawn with.
    pub const GLYPH: &'static str = "●";

    /// The dot's color in `theme`.
    #[must_use]
    pub const fn color(self, theme: &Theme) -> Color {
        let colors = &theme.colors;
        match self {
            Self::Ok => colors.success,
            Self::Busy => colors.warning,
            Self::Waiting => colors.muted,
            Self::Failed => colors.error,
            Self::Off => colors.faint,
        }
    }

    /// The dot as a span: the glyph in its color, nothing else styled.
    #[must_use]
    pub fn span(self, theme: &Theme) -> Span<'static> {
        Span::styled(Self::GLYPH, Style::default().fg(self.color(theme)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    #[test]
    fn each_state_has_its_own_color_and_no_weight() {
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
            assert_eq!(span.content, "●");
            assert_eq!(span.style.fg, Some(dot.color(&theme)));
            assert!(!span.style.add_modifier.contains(Modifier::BOLD));
            for other in &all[at + 1..] {
                assert_ne!(dot.color(&theme), other.color(&theme), "{dot:?} {other:?}");
            }
        }
    }
}
