//! Responsive layout breakpoints.

use serde::{Deserialize, Serialize};

/// Layout mode based on terminal width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LayoutMode {
    /// Compact mode (< 60 chars): minimal chrome, no borders
    Compact,
    /// Normal mode (60-79 chars): standard layout
    #[default]
    Normal,
    /// Comfortable mode (80-119 chars): full borders, icons
    Comfortable,
    /// Wide mode (>= 120 chars): side panels available
    Wide,
}

impl LayoutMode {
    /// Get the layout mode for a given terminal width.
    #[must_use]
    pub const fn from_width(width: u16) -> Self {
        match width {
            0..=59 => Self::Compact,
            60..=79 => Self::Normal,
            80..=119 => Self::Comfortable,
            _ => Self::Wide,
        }
    }

    /// Check if this mode supports side panels.
    #[must_use]
    pub const fn has_side_panel(self) -> bool {
        matches!(self, Self::Wide)
    }

    /// Check if this mode shows full borders.
    #[must_use]
    pub const fn has_full_borders(self) -> bool {
        !matches!(self, Self::Compact)
    }

    /// Check if this mode shows icons.
    #[must_use]
    pub const fn has_icons(self) -> bool {
        matches!(self, Self::Comfortable | Self::Wide)
    }

    /// Get the message area width fraction.
    #[must_use]
    pub const fn message_width_fraction(self) -> f32 {
        match self {
            Self::Compact | Self::Normal | Self::Comfortable => 1.0,
            Self::Wide => 0.7, // 70% for messages, 30% for side panel
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layout_mode_from_width() {
        assert_eq!(LayoutMode::from_width(40), LayoutMode::Compact);
        assert_eq!(LayoutMode::from_width(59), LayoutMode::Compact);
        assert_eq!(LayoutMode::from_width(60), LayoutMode::Normal);
        assert_eq!(LayoutMode::from_width(79), LayoutMode::Normal);
        assert_eq!(LayoutMode::from_width(80), LayoutMode::Comfortable);
        assert_eq!(LayoutMode::from_width(119), LayoutMode::Comfortable);
        assert_eq!(LayoutMode::from_width(120), LayoutMode::Wide);
        assert_eq!(LayoutMode::from_width(200), LayoutMode::Wide);
    }

    #[test]
    fn test_layout_mode_features() {
        assert!(!LayoutMode::Compact.has_side_panel());
        assert!(LayoutMode::Wide.has_side_panel());

        assert!(!LayoutMode::Compact.has_full_borders());
        assert!(LayoutMode::Normal.has_full_borders());

        assert!(!LayoutMode::Normal.has_icons());
        assert!(LayoutMode::Comfortable.has_icons());
    }
}
