use super::*;
use crate::components::{
    agent_button_offset, Activity, Message, MessageRole, ToolStatus, ToolStep,
};
use crate::events::{ActivityData, CallUsage, MessageData, ModelOptionView, UiCommand, UiEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use tokio::sync::mpsc;

mod support;
use support::*;

mod activity_blocks;
mod agent_chats;
mod agent_records;
mod approvals_and_mentions;
mod basics;
mod channel_chat;
mod channel_form;
mod chat_scrollbar;
mod chat_switching_and_channels_tab;
mod commands_and_shell;
mod ctrl_g_editor;
mod drag_and_click;
mod folding;
mod model_selector;
mod multi_line_input;
mod pasted_images;
mod projects_and_agent_buttons;
mod roster;
mod scrolling;
mod session_modes_and_plans;
mod slash_commands;
