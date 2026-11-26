use super::*;
use crate::components::{LiveOutput, MORE_PAGE_LINES};
use crate::themes::Theme;

mod support;
// Shadows the renderer's own `activity_lines` brought in by `super::*`.
use support::activity_lines;
use support::*;

mod expanded;
mod headers;
mod layout;
mod live;
