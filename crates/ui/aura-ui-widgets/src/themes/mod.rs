//! Theme system for the terminal UI.
//!
//! Provides customizable color schemes and styling options.
//!
//! # Approved Colors (Cyber Theme)
//!
//! | Color       | Hex Code | Constant   | Usage                              |
//! |-------------|----------|------------|------------------------------------|
//! | Cyan/Green  | #01f4cb  | `CYAN`     | The accent: status, success        |
//! | Blue        | #01a4f4  | `BLUE`     | Info, links                        |
//! | Purple      | #cb01f4  | `PURPLE`   | Code block keywords                |
//! | Red         | #e06c75  | `RED`      | Errors, danger                     |
//! | White       | #ffffff  | `WHITE`    | Titles, emphasized text            |
//! | Gray        | #808080  | `GRAY`     | Muted text, secondary info         |
//! | Dim gray    | #5a5a5a  | `DIM_GRAY` | Borders, markers, the footer       |
//! | Void        | #0a0a0a  | `VOID`     | Backdrop under a dialog            |
//! | Graphite    | #191919  | `BLACK`    | Background                         |

mod cyber;
mod theme;

pub use cyber::{cyber_theme, BLACK, BLUE, CYAN, DIM_GRAY, GRAY, PURPLE, RED, VOID, WHITE};
pub use theme::{BorderStyle, Theme, ThemeColors};
