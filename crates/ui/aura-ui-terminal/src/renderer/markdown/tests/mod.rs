use super::inline::{parse_inline, parse_linked};
use super::render_markdown;
use super::wrap::wrap_spans;
use crate::renderer::text::Measure;
use crate::Theme;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

mod support;
use support::*;

mod blocks;
mod fields;
mod inline;
mod links;
mod tables;
mod wrapping;
