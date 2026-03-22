//! Theme struct definition and loading.

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

/// Terminal color theme.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Theme name
    pub name: String,
    /// Color palette
    pub colors: ThemeColors,
    /// Border style preference
    pub border_style: BorderStyle,
    /// Whether to show icons
    pub show_icons: bool,
    /// Whether to animate spinners
    pub animate_spinners: bool,
    /// Whether to show timestamps
    pub show_timestamps: bool,
}

impl Theme {
    /// Create a new theme with the given name and colors.
    #[must_use]
    pub fn new(name: impl Into<String>, colors: ThemeColors) -> Self {
        Self {
            name: name.into(),
            colors,
            border_style: BorderStyle::Rounded,
            show_icons: true,
            animate_spinners: true,
            show_timestamps: true,
        }
    }

    /// Create the default cyber theme.
    #[must_use]
    pub fn cyber() -> Self {
        super::cyber::cyber_theme()
    }

    /// Create a minimal theme.
    #[must_use]
    pub fn minimal() -> Self {
        Self::new(
            "minimal",
            ThemeColors {
                background: Color::Rgb(26, 26, 26),
                foreground: Color::White,
                primary: Color::White,
                secondary: Color::Rgb(136, 136, 136),
                success: Color::Green,
                warning: Color::Yellow,
                error: Color::Red,
                pending: Color::Rgb(136, 136, 136),
                muted: Color::Rgb(136, 136, 136),
            },
        )
    }

    /// Create a matrix green theme.
    #[must_use]
    pub fn matrix() -> Self {
        Self::new(
            "matrix",
            ThemeColors {
                background: Color::Black,
                foreground: Color::Rgb(0, 255, 0),
                primary: Color::Rgb(0, 255, 0),
                secondary: Color::Rgb(0, 128, 0),
                success: Color::Rgb(0, 255, 0),
                warning: Color::Rgb(0, 200, 0),
                error: Color::Rgb(200, 0, 0),
                pending: Color::Rgb(0, 180, 0),
                muted: Color::Rgb(0, 100, 0),
            },
        )
    }

    /// Create a synthwave theme.
    #[must_use]
    pub fn synthwave() -> Self {
        Self::new(
            "synthwave",
            ThemeColors {
                background: Color::Rgb(26, 26, 46),
                foreground: Color::White,
                primary: Color::Rgb(255, 46, 151),  // hot pink
                secondary: Color::Rgb(0, 212, 255), // electric blue
                success: Color::Rgb(57, 255, 20),   // neon green
                warning: Color::Rgb(255, 230, 109), // sunset yellow
                error: Color::Rgb(255, 20, 147),    // deep pink
                pending: Color::Rgb(180, 100, 255), // lavender
                muted: Color::Rgb(136, 136, 136),
            },
        )
    }

    /// Get a theme by name.
    #[must_use]
    pub fn by_name(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "matrix" => Self::matrix(),
            "synthwave" => Self::synthwave(),
            "minimal" => Self::minimal(),
            // Default to cyber for unknown themes
            _ => Self::cyber(),
        }
    }

    /// Set the border style.
    #[must_use]
    pub const fn with_border_style(mut self, style: BorderStyle) -> Self {
        self.border_style = style;
        self
    }

    /// Set whether to show timestamps.
    #[must_use]
    pub const fn with_timestamps(mut self, show: bool) -> Self {
        self.show_timestamps = show;
        self
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::cyber()
    }
}

/// Color palette for a theme.
///
/// # Approved Colors (Cyber Theme)
///
/// | Color       | Hex Code | Usage                              |
/// |-------------|----------|------------------------------------|
/// | Cyan/Green  | #01f4cb  | Success states, neon accents       |
/// | Blue        | #01a4f4  | Primary accent, info, provisioning |
/// | Purple      | #cb01f4  | Pending states, secondary accent   |
/// | Red         | #f4012a  | Errors, danger                     |
/// | White       | #ffffff  | Primary text                       |
/// | Gray        | #888888  | Muted text, secondary info         |
/// | Black       | #0d0d0d  | Background                         |
#[derive(Debug, Clone)]
pub struct ThemeColors {
    /// Background color (#0d0d0d black)
    pub background: Color,
    /// Foreground/primary text color (#ffffff white)
    pub foreground: Color,
    /// Primary accent color - blue (#01a4f4)
    pub primary: Color,
    /// Secondary accent color - purple (#cb01f4)
    pub secondary: Color,
    /// Success color - cyan/green (#01f4cb)
    pub success: Color,
    /// Warning/info color - blue (#01a4f4)
    pub warning: Color,
    /// Error/danger color - red (#f4012a)
    pub error: Color,
    /// Pending state color - purple (#cb01f4)
    pub pending: Color,
    /// Muted text color - gray (#888888)
    pub muted: Color,
}

/// Border style for UI elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BorderStyle {
    /// Single line borders: ┌─┐
    Plain,
    /// Double line borders: ╔═╗
    Double,
    /// Rounded corners: ╭─╮
    #[default]
    Rounded,
    /// Heavy borders: ┏━┓
    Heavy,
    /// ASCII only: +-+
    Ascii,
}

impl BorderStyle {
    /// Get the ratatui border type.
    #[must_use]
    pub const fn to_border_type(self) -> ratatui::widgets::BorderType {
        match self {
            Self::Plain | Self::Ascii => ratatui::widgets::BorderType::Plain,
            Self::Double => ratatui::widgets::BorderType::Double,
            Self::Rounded => ratatui::widgets::BorderType::Rounded,
            Self::Heavy => ratatui::widgets::BorderType::Thick,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_creation() {
        let theme = Theme::cyber();
        assert_eq!(theme.name, "cyber");
    }

    #[test]
    fn test_theme_by_name() {
        let theme = Theme::by_name("matrix");
        assert_eq!(theme.name, "matrix");

        let theme = Theme::by_name("unknown");
        assert_eq!(theme.name, "cyber"); // defaults to cyber
    }

    #[test]
    fn test_border_style_conversion() {
        let style = BorderStyle::Rounded;
        let _border_type = style.to_border_type();
    }

    #[test]
    fn test_theme_builder_methods() {
        let theme = Theme::cyber()
            .with_border_style(BorderStyle::Double)
            .with_timestamps(false);

        assert_eq!(theme.border_style, BorderStyle::Double);
        assert!(!theme.show_timestamps);
    }

    // ========================================================================
    // Additional Theme Tests
    // ========================================================================

    #[test]
    fn test_all_named_themes() {
        let cyber = Theme::by_name("cyber");
        let matrix = Theme::by_name("matrix");
        let synthwave = Theme::by_name("synthwave");
        let minimal = Theme::by_name("minimal");

        assert_eq!(cyber.name, "cyber");
        assert_eq!(matrix.name, "matrix");
        assert_eq!(synthwave.name, "synthwave");
        assert_eq!(minimal.name, "minimal");
    }

    #[test]
    fn test_theme_case_insensitive() {
        let theme1 = Theme::by_name("MATRIX");
        let theme2 = Theme::by_name("Matrix");
        let theme3 = Theme::by_name("matrix");

        assert_eq!(theme1.name, "matrix");
        assert_eq!(theme2.name, "matrix");
        assert_eq!(theme3.name, "matrix");
    }

    #[test]
    fn test_theme_default() {
        let theme = Theme::default();
        assert_eq!(theme.name, "cyber");
    }

    #[test]
    fn test_all_border_styles() {
        let plain = BorderStyle::Plain;
        let double = BorderStyle::Double;
        let rounded = BorderStyle::Rounded;
        let heavy = BorderStyle::Heavy;
        let ascii = BorderStyle::Ascii;

        // Just verify they convert without panicking
        let _ = plain.to_border_type();
        let _ = double.to_border_type();
        let _ = rounded.to_border_type();
        let _ = heavy.to_border_type();
        let _ = ascii.to_border_type();
    }

    #[test]
    fn test_theme_colors_are_set() {
        let theme = Theme::cyber();

        // Verify all colors are non-default (not black for everything)
        // This is a basic sanity check
        assert_ne!(theme.colors.primary, theme.colors.error);
        assert_ne!(theme.colors.success, theme.colors.error);
    }

    #[test]
    fn test_custom_theme() {
        let colors = ThemeColors {
            background: Color::Black,
            foreground: Color::White,
            primary: Color::Blue,
            secondary: Color::Cyan,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            pending: Color::Magenta,
            muted: Color::Gray,
        };

        let theme = Theme::new("custom", colors);
        assert_eq!(theme.name, "custom");
        assert!(theme.show_icons);
        assert!(theme.animate_spinners);
        assert!(theme.show_timestamps);
    }

    #[test]
    fn test_matrix_theme_is_green() {
        let theme = Theme::matrix();
        // Matrix theme should have green colors
        match theme.colors.primary {
            Color::Rgb(r, g, b) => {
                // Green should be the dominant color
                assert!(g > r && g > b);
            }
            _ => panic!("Expected RGB color for matrix theme"),
        }
    }

    #[test]
    fn test_synthwave_has_distinct_colors() {
        let theme = Theme::synthwave();
        // Synthwave should have distinct primary and secondary colors
        assert_ne!(theme.colors.primary, theme.colors.secondary);
    }

    #[test]
    fn test_border_style_default() {
        let style = BorderStyle::default();
        assert_eq!(style, BorderStyle::Rounded);
    }

    #[test]
    fn test_switch_theme_changes_colors() {
        let cyber = Theme::cyber();
        let matrix = Theme::matrix();
        assert_ne!(cyber.colors.primary, matrix.colors.primary);
        assert_ne!(cyber.colors.background, matrix.colors.background);
    }

    #[test]
    fn test_switch_theme_preserves_builder_settings() {
        let theme = Theme::matrix()
            .with_border_style(BorderStyle::Heavy)
            .with_timestamps(false);
        assert_eq!(theme.name, "matrix");
        assert_eq!(theme.border_style, BorderStyle::Heavy);
        assert!(!theme.show_timestamps);
    }

    #[test]
    fn test_all_themes_have_distinct_names() {
        let names: Vec<String> = ["cyber", "matrix", "synthwave", "minimal"]
            .iter()
            .map(|n| Theme::by_name(n).name)
            .collect();
        let unique: std::collections::HashSet<&str> = names.iter().map(|s| s.as_str()).collect();
        assert_eq!(names.len(), unique.len());
    }

    #[test]
    fn test_minimal_theme_colors() {
        let theme = Theme::minimal();
        assert_eq!(theme.colors.foreground, Color::White);
        assert_eq!(theme.colors.success, Color::Green);
        assert_eq!(theme.colors.error, Color::Red);
    }

    #[test]
    fn test_border_style_roundtrip_serde() {
        let styles = [
            BorderStyle::Plain,
            BorderStyle::Double,
            BorderStyle::Rounded,
            BorderStyle::Heavy,
            BorderStyle::Ascii,
        ];
        for style in styles {
            let json = serde_json::to_string(&style).unwrap();
            let back: BorderStyle = serde_json::from_str(&json).unwrap();
            assert_eq!(style, back);
        }
    }
}
