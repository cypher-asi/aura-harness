//! Default cyber theme: graphite neutrals with one green accent.
//!
//! # Approved Color Palette
//!
//! | Color       | Hex Code | RGB            | Usage                                   |
//! |-------------|----------|----------------|-----------------------------------------|
//! | Cyan/Green  | #01f4cb  | (1, 244, 203)  | Status glyphs, success, diff additions  |
//! | Blue        | #01a4f4  | (1, 164, 244)  | Info, links, mode badges                |
//! | Purple      | #cb01f4  | (203, 1, 244)  | Code block keywords                     |
//! | Red         | #e06c75  | (224, 108, 117)| Errors, danger                          |
//! | White       | #ffffff  | (255, 255, 255)| Titles, the user's text, the current item |
//! | Light gray  | #f2f2f2  | (242, 242, 242)| Body text of chat replies               |
//! | Silver      | #b3b3b3  | (179, 179, 179)| Secondary text, the verb of a step      |
//! | Gray        | #808080  | (128, 128, 128)| Muted text, focused borders             |
//! | Dim gray    | #5a5a5a  | (90, 90, 90)   | Borders, markers, the footer            |
//! | Graphite    | #191919  | (25, 25, 25)   | Background                              |
//! | Dark gray   | #242424  | (36, 36, 36)   | List cell, the user's messages, code    |
//! | Charcoal    | #2e2e2e  | (46, 46, 46)   | List cell under the pointer             |
//! | Slate       | #383838  | (56, 56, 56)   | Selected row, active tab                |

use super::{BorderStyle, Theme, ThemeColors};
use ratatui::style::Color;

// ============================================================================
// APPROVED COLOR CONSTANTS
// ============================================================================

/// Cyan/Green - Status glyphs, success (#01f4cb)
pub const CYAN: Color = Color::Rgb(1, 244, 203);

/// Blue - Info, links (#01a4f4)
pub const BLUE: Color = Color::Rgb(1, 164, 244);

/// Purple - Code block keywords (#cb01f4)
pub const PURPLE: Color = Color::Rgb(203, 1, 244);

/// Red - Errors, danger (#e06c75)
pub const RED: Color = Color::Rgb(224, 108, 117);

/// White - Titles and emphasized text (#ffffff)
pub const WHITE: Color = Color::White;

/// Light gray - Body text of chat replies (#f2f2f2)
pub const LIGHT_GRAY: Color = Color::Rgb(242, 242, 242);

/// Silver - Secondary text (#b3b3b3)
pub const SILVER: Color = Color::Rgb(179, 179, 179);

/// Gray - Muted text, secondary info (#808080)
pub const GRAY: Color = Color::Rgb(128, 128, 128);

/// Dim gray - Borders, markers and the footer (#5a5a5a)
pub const DIM_GRAY: Color = Color::Rgb(90, 90, 90);

/// Graphite - Background (#191919)
pub const BLACK: Color = Color::Rgb(25, 25, 25);

/// Dark gray - List cell, shaded messages (#242424)
pub const DARK_GRAY: Color = Color::Rgb(36, 36, 36);

/// Charcoal - List cell under the pointer (#2e2e2e)
pub const GRAPHITE: Color = Color::Rgb(46, 46, 46);

/// Slate - Selected row and active tab (#383838)
pub const SLATE: Color = Color::Rgb(56, 56, 56);

// ============================================================================
// THEME
// ============================================================================

/// Create the default cyber theme.
///
/// Text and chrome are shades of gray, so the screen reads by brightness:
/// white for what matters now, light gray for replies, gray for what
/// surrounds them, dim gray for borders. The green accent marks status
/// (working, ready, connected, success) and nothing else.
///
/// Purple is reserved for syntax-highlighted keywords in code blocks.
#[must_use]
pub fn cyber_theme() -> Theme {
    Theme {
        name: "cyber".to_string(),
        colors: ThemeColors {
            // Base colors
            background: BLACK, // #191919 - graphite
            foreground: WHITE, // #ffffff - white text
            text: LIGHT_GRAY,  // #f2f2f2 - chat reply body

            // Neutral emphasis
            primary: WHITE,    // #ffffff - titles, the user's text
            secondary: SILVER, // #b3b3b3 - secondary text

            // Semantic colors
            success: CYAN,  // #01f4cb - the accent: status and success
            warning: BLUE,  // #01a4f4 - info/warning
            error: RED,     // #e06c75 - errors/danger
            pending: WHITE, // #ffffff - pending states

            // Muted text and borders
            muted: GRAY,     // #808080 - gray
            faint: DIM_GRAY, // #5a5a5a - dim gray

            // List cells
            surface: DARK_GRAY,      // #242424 - dark gray
            surface_hover: GRAPHITE, // #2e2e2e - charcoal
            selection: SLATE,        // #383838 - slate
            on_selection: WHITE,     // #ffffff - white
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
        assert_eq!(theme.colors.primary, WHITE);
        assert_eq!(theme.colors.success, CYAN);
        assert_eq!(theme.colors.error, RED);
        assert_eq!(theme.colors.secondary, SILVER);
        assert_eq!(theme.colors.pending, WHITE);
        assert_eq!(theme.colors.selection, SLATE);
        assert_eq!(theme.colors.on_selection, WHITE);
    }

    #[test]
    fn test_color_constants() {
        assert_eq!(CYAN, Color::Rgb(1, 244, 203));
        assert_eq!(BLUE, Color::Rgb(1, 164, 244));
        assert_eq!(PURPLE, Color::Rgb(203, 1, 244));
        assert_eq!(RED, Color::Rgb(224, 108, 117));
    }
}
