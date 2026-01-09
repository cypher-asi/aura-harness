//! Default cyber theme with neon colors.

use super::{BorderStyle, Theme, ThemeColors};
use ratatui::style::Color;

/// Create the default cyber theme.
///
/// Colors based on classic cyberpunk aesthetics:
/// - Neon Cyan: Primary accent, AI responses, ready status
/// - Magenta: Secondary accent, tool calls
/// - Deep Black: Background
#[must_use]
pub fn cyber_theme() -> Theme {
    Theme {
        name: "cyber".to_string(),
        colors: ThemeColors {
            // Base colors
            background: Color::Rgb(13, 13, 13),   // #0D0D0D - deep black
            foreground: Color::White,             // #FFFFFF - white text

            // Primary colors
            primary: Color::Rgb(1, 244, 203),     // #01F4CB - neon cyan
            secondary: Color::Rgb(255, 0, 255),   // #FF00FF - magenta

            // Semantic colors
            success: Color::Rgb(1, 244, 203),     // #01F4CB - neon cyan (same as primary)
            warning: Color::Rgb(255, 191, 0),     // #FFBF00 - amber
            error: Color::Rgb(255, 20, 147),      // #FF1493 - hot pink

            // Muted - light gray for user message text
            muted: Color::Rgb(180, 180, 180),     // #B4B4B4 - light gray
        },
        border_style: BorderStyle::Rounded,
        show_icons: true,
        animate_spinners: true,
        show_timestamps: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cyber_theme() {
        let theme = cyber_theme();
        assert_eq!(theme.name, "cyber");
        assert_eq!(theme.colors.primary, Color::Rgb(0, 255, 255));
    }
}
