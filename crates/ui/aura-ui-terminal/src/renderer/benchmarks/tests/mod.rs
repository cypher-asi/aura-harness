use super::form::form_lines;
use super::format::*;
use super::*;
use crate::events::*;
use crate::events::{BenchBaseline, BenchJudge, BenchRun, UiCommand};
use crate::renderer::text::display_width;
use ratatui::text::Line;
use ratatui::{backend::TestBackend, Terminal};

mod support;
use support::*;

mod detail;
mod layout;
mod pricing;
mod running;
