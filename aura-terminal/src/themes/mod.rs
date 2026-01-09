//! Theme system for the terminal UI.
//!
//! Provides customizable color schemes and styling options.

mod cyber;
mod theme;

pub use cyber::cyber_theme;
pub use theme::{BorderStyle, Theme, ThemeColors};
