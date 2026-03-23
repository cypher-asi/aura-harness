//! Theme system for the terminal UI.
//!
//! Provides customizable color schemes and styling options.
//!
//! # Approved Colors (Cyber Theme)
//!
//! | Color       | Hex Code | Constant | Usage                              |
//! |-------------|----------|----------|------------------------------------|
//! | Cyan/Green  | #01f4cb  | `CYAN`   | Success states, neon accents       |
//! | Blue        | #01a4f4  | `BLUE`   | Primary accent, info, provisioning |
//! | Purple      | #cb01f4  | `PURPLE` | Pending states, secondary accent   |
//! | Red         | #f4012a  | `RED`    | Errors, danger                     |
//! | White       | #ffffff  | `WHITE`  | Primary text                       |
//! | Gray        | #888888  | `GRAY`   | Muted text, secondary info         |
//! | Black       | #0d0d0d  | `BLACK`  | Background                         |

mod cyber;
mod theme;

pub use cyber::{cyber_theme, BLACK, BLUE, CYAN, GRAY, PURPLE, RED, WHITE};
pub use theme::{BorderStyle, Theme, ThemeColors};
