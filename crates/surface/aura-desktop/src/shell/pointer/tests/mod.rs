use super::*;
use crate::shell::keys::Press;
use crate::shell::tests::{stage, Stage};
use crate::shell::WINDOW;
use aura_ui::events::UiCommand;
use aura_ui_terminal::FLOW_MIN_ZOOM;
use aura_ui_terminal::{ChatTab, Pane, PaneChrome, PaneTab};
use std::time::Duration;
use winit::dpi::PhysicalPosition;
use winit::keyboard::{ModifiersState, NamedKey};

mod canvas;
mod chat;
mod menus;
mod support;
mod window;
