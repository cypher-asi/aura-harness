//! Tests of the queued-messages panel: mouse, keyboard and editing.

use super::queue_panel::QueueRowArea;
use super::App;
use crate::events::{AgentSummary, QueuedItemView, UiCommand, UiEvent};
use crate::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use tokio::sync::mpsc;

/// Enough of a PNG for the input to take it as one.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nimage";

mod support;
use support::*;

mod editing;
mod keyboard;
mod mouse;
mod queue_sync;
