//! Default cyber theme with neon colors.
//!
//! # Approved Color Palette
//!
//! | Color       | Hex Code | RGB            | Usage                              |
//! |-------------|----------|----------------|-------------------------------------|
//! | Cyan/Green  | #01f4cb  | (1, 244, 203)  | Success states, neon accents        |
//! | Blue        | #01a4f4  | (1, 164, 244)  | Primary accent, info, provisioning  |
//! | Purple      | #cb01f4  | (203, 1, 244)  | Pending states, secondary accent    |
//! | Red         | #f4012a  | (244, 1, 42)   | Errors, danger                      |
//! | White       | #ffffff  | (255, 255, 255)| Primary text                        |
//! | Gray        | #888888  | (136, 136, 136)| Muted text, secondary info          |
//! | Black       | #0d0d0d  | (13, 13, 13)   | Background                          |

use super::{BorderStyle, Theme, ThemeColors};
use ratatui::style::Color;

// ============================================================================
// APPROVED COLOR CONSTANTS
// ============================================================================

/// Cyan/Green - Success states, neon accents (#01f4cb)
pub const CYAN: Color = Color::Rgb(1, 244, 203);

/// Blue - Primary accent, info, provisioning (#01a4f4)
pub const BLUE: Color = Color::Rgb(1, 164, 244);

/// Purple - Pending states, secondary accent (#cb01f4)
pub const PURPLE: Color = Color::Rgb(203, 1, 244);

/// Red - Errors, danger (#f4012a)
pub const RED: Color = Color::Rgb(244, 1, 42);

/// White - Primary text (#ffffff)
pub const WHITE: Color = Color::White;

/// Gray - Muted text, secondary info (#888888)
pub const GRAY: Color = Color::Rgb(136, 136, 136);

/// Black - Background (#0d0d0d)
pub const BLACK: Color = Color::Rgb(13, 13, 13);

// ============================================================================
// THEME
// ============================================================================

/// Create the default cyber theme.
///
/// Uses only the approved color palette:
/// - Cyan (#01f4cb): Success, neon accents
/// - Blue (#01a4f4): Primary accent, info
/// - Purple (#cb01f4): Pending, secondary accent
/// - Red (#f4012a): Errors, danger
/// - White/Gray/Black: Text and backgrounds
#[must_use]
pub fn cyber_theme() -> Theme {
    Theme {
        name: "cyber".to_string(),
        colors: ThemeColors {
            // Base colors
            background: BLACK, // #0d0d0d - deep black
            foreground: WHITE, // #ffffff - white text

            // Accent colors
            primary: CYAN,     // #01f4cb - cyan (primary accent)
            secondary: PURPLE, // #cb01f4 - purple (secondary accent)

            // Semantic colors
            success: CYAN,   // #01f4cb - cyan/green (success)
            warning: BLUE,   // #01a4f4 - blue (info/warning)
            error: RED,      // #f4012a - red (errors/danger)
            pending: PURPLE, // #cb01f4 - purple (pending states)

            // Muted text
            muted: GRAY, // #888888 - gray
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
        assert_eq!(theme.colors.primary, CYAN);
        assert_eq!(theme.colors.success, CYAN);
        assert_eq!(theme.colors.error, RED);
        assert_eq!(theme.colors.pending, PURPLE);
    }

    #[test]
    fn test_color_constants() {
        assert_eq!(CYAN, Color::Rgb(1, 244, 203));
        assert_eq!(BLUE, Color::Rgb(1, 164, 244));
        assert_eq!(PURPLE, Color::Rgb(203, 1, 244));
        assert_eq!(RED, Color::Rgb(244, 1, 42));
    }
}
