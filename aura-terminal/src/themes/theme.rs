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
#[derive(Debug, Clone)]
pub struct ThemeColors {
    /// Background color
    pub background: Color,
    /// Foreground (text) color
    pub foreground: Color,
    /// Primary accent color (AI responses)
    pub primary: Color,
    /// Secondary accent color (tool calls)
    pub secondary: Color,
    /// Success color
    pub success: Color,
    /// Warning color
    pub warning: Color,
    /// Error color
    pub error: Color,
    /// Muted text color
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
}
