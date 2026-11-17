//! Default cyber theme with neon colors.
//!
//! # Approved Color Palette
//!
//! | Color       | Hex Code | RGB            | Usage                              |
//! |-------------|----------|----------------|-------------------------------------|
//! | Cyan/Green  | #01f4cb  | (1, 244, 203)  | Success, neon accents, selected row |
//! | Blue        | #01a4f4  | (1, 164, 244)  | Primary accent, info, provisioning  |
//! | Purple      | #cb01f4  | (203, 1, 244)  | Code block keywords                 |
//! | Red         | #f4012a  | (244, 1, 42)   | Errors, danger                      |
//! | White       | #ffffff  | (255, 255, 255)| Text, secondary accent, pending     |
//! | Light gray  | #e6e6e6  | (230, 230, 230)| Body text of chat replies           |
//! | Gray        | #888888  | (136, 136, 136)| Muted text, secondary info          |
//! | Black       | #0d0d0d  | (13, 13, 13)   | Background, text on selection       |
//! | Dark gray   | #1e1e1e  | (30, 30, 30)   | List cell                           |
//! | Graphite    | #2a2a2a  | (42, 42, 42)   | List cell under the pointer         |

use super::{BorderStyle, Theme, ThemeColors};
use ratatui::style::Color;

// ============================================================================
// APPROVED COLOR CONSTANTS
// ============================================================================

/// Cyan/Green - Success states, neon accents (#01f4cb)
pub const CYAN: Color = Color::Rgb(1, 244, 203);

/// Blue - Primary accent, info, provisioning (#01a4f4)
pub const BLUE: Color = Color::Rgb(1, 164, 244);

/// Purple - Code block keywords (#cb01f4)
pub const PURPLE: Color = Color::Rgb(203, 1, 244);

/// Red - Errors, danger (#f4012a)
pub const RED: Color = Color::Rgb(244, 1, 42);

/// White - Primary text (#ffffff)
pub const WHITE: Color = Color::White;

/// Light gray - Body text of chat replies (#e6e6e6)
pub const LIGHT_GRAY: Color = Color::Rgb(230, 230, 230);

/// Gray - Muted text, secondary info (#888888)
pub const GRAY: Color = Color::Rgb(136, 136, 136);

/// Black - Background (#0d0d0d)
pub const BLACK: Color = Color::Rgb(13, 13, 13);

/// Dark gray - List cell (#1e1e1e)
pub const DARK_GRAY: Color = Color::Rgb(30, 30, 30);

/// Graphite - List cell under the pointer (#2a2a2a)
pub const GRAPHITE: Color = Color::Rgb(42, 42, 42);

// ============================================================================
// THEME
// ============================================================================

/// Create the default cyber theme.
///
/// Uses only the approved color palette:
/// - Cyan (#01f4cb): Success, neon accents, selected list row
/// - Blue (#01a4f4): Info
/// - Red (#f4012a): Errors, danger
/// - White: Text, secondary accent, pending states
/// - Gray/Black: Muted text and backgrounds
///
/// Purple is reserved for syntax-highlighted keywords in code blocks.
#[must_use]
pub fn cyber_theme() -> Theme {
    Theme {
        name: "cyber".to_string(),
        colors: ThemeColors {
            // Base colors
            background: BLACK, // #0d0d0d - deep black
            foreground: WHITE, // #ffffff - white text
            text: LIGHT_GRAY,  // #e6e6e6 - chat reply body

            // Accent colors
            primary: CYAN,    // #01f4cb - cyan (primary accent)
            secondary: WHITE, // #ffffff - white (secondary accent)

            // Semantic colors
            success: CYAN,  // #01f4cb - cyan/green (success)
            warning: BLUE,  // #01a4f4 - blue (info/warning)
            error: RED,     // #f4012a - red (errors/danger)
            pending: WHITE, // #ffffff - white (pending states)

            // Muted text
            muted: GRAY, // #888888 - gray

            // List cells
            surface: DARK_GRAY,      // #1e1e1e - dark gray
            surface_hover: GRAPHITE, // #2a2a2a - graphite
            selection: CYAN,         // #01f4cb - cyan, same as the primary accent
            on_selection: BLACK,     // #0d0d0d - black
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
        assert_eq!(theme.colors.secondary, WHITE);
        assert_eq!(theme.colors.pending, WHITE);
    }

    #[test]
    fn test_color_constants() {
        assert_eq!(CYAN, Color::Rgb(1, 244, 203));
        assert_eq!(BLUE, Color::Rgb(1, 164, 244));
        assert_eq!(PURPLE, Color::Rgb(203, 1, 244));
        assert_eq!(RED, Color::Rgb(244, 1, 42));
    }
}
