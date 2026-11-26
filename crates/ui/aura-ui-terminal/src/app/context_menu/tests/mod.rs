use super::*;
use crate::app::{AgentsTab, App, AppState, PanelFocus, ProjectListRow};
use crate::events::{AgentSummary, UiCommand, UiEvent};
use crossterm::event::{KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use tokio::sync::mpsc;

mod support;
use support::*;

mod menus;
mod submenu_over_menu;
mod workspace_moves;
